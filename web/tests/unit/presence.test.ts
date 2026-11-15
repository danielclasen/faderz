import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore } from '../../src/lib/state/mixer.svelte';
import { REQUEST_INTERVAL_MS } from '../../src/lib/contract/protocol';
import type { ToWorker } from '../../src/lib/sync/messages';
import type { Owner } from '../../src/lib/contract/types';
import { exampleShape, initialState } from '../../dev/mock/shape';
import { handleTag, ownerHandle } from '../../src/lib/net/handle';

const ME = ownerHandle('bWUtc2Vzc2lvbi0xMjM0NTY3');
const YOU: Owner = { principal: 'anonymous', handle: ME, kind: 'ui', connection: 'c1' };
const OTHER: Owner = { principal: 'anonymous', handle: ownerHandle('b3RoZXItc2Vzc2lvbi0xMjM'), kind: 'ui', connection: 'c9' };
const NAMED: Owner = { principal: 'daniel', handle: ownerHandle('bmFtZWQtc2Vzc2lvbi0xMjM'), kind: 'ui', connection: 'c7' };
const PATH = '/mixMinus/mm1/interrupt/active';

function store() {
  const s = new MixerStore();
  const sent: ToWorker[] = [];
  s.attach({ post: (m) => sent.push(m) });
  const shape = exampleShape();
  s.apply({
    type: 'patch',
    input: 0,
    snapshot: { shape, state: initialState(shape), owners: { [PATH]: OTHER, '/channels/ch1/fader': YOU } },
    changes: [],
    owners: [],
    status: { ...INITIAL_STATUS, link: 'Live', handle: ME, you: YOU },
  });
  return { s, sent };
}

describe('control requests', () => {
  it('asks the holder of a control once per interval, and says so only once the worker sent it', () => {
    const { s, sent } = store();
    expect(s.requestControl(PATH, 1000)).toBe(true);
    expect(sent).toEqual([{ type: 'request', path: PATH }]);
    expect(s.notices).toEqual([]);
    s.apply({ type: 'patch', input: 1, changes: [], owners: [], notices: [{ kind: 'requested', path: PATH, owner: OTHER }] });
    expect(s.notices.at(-1)?.text).toMatch(/^Asked web client \S+ for N-1 /);
    expect(s.requestControl(PATH, 1000 + REQUEST_INTERVAL_MS - 1)).toBe(false);
    expect(sent).toHaveLength(1);
    expect(s.requestControl(PATH, 1000 + REQUEST_INTERVAL_MS)).toBe(true);
    expect(sent).toHaveLength(2);
  });

  it('asks nobody for a control that is free or its own', () => {
    const { s, sent } = store();
    expect(s.requestControl('/channels/ch2/fader', 0)).toBe(false);
    expect(s.requestControl('/channels/ch1/fader', 0)).toBe(false);
    expect(sent).toEqual([]);
  });

  it('shows a request for a held control as a notice naming the asker', () => {
    const { s } = store();
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], notices: [{ kind: 'request', path: '/channels/ch1/fader', from: OTHER }] });
    expect(s.notices.at(-1)).toMatchObject({ kind: 'request', text: expect.stringMatching(/^web client \S+ asks for Pres 1 fader$/) });
  });
});

describe('giving a control up on request', () => {
  it('ends a talk this session holds and releases it, and the notice carries the control', () => {
    const { s, sent } = store();
    const talk = '/mixMinus/mm2/interrupt/active';
    s.apply({ type: 'patch', input: 0, changes: [[talk, true]], owners: [[talk, YOU]], notices: [{ kind: 'request', path: talk, from: OTHER }] });
    expect(s.notices.at(-1)).toMatchObject({ kind: 'request', path: talk });
    s.giveUp(talk);
    expect(sent).toEqual([
      { type: 'set', path: talk, value: false },
      { type: 'release', path: talk },
    ]);
  });

  it('releases anything else without writing it, and gives up nothing it does not hold', () => {
    const { s, sent } = store();
    s.giveUp('/channels/ch1/fader');
    s.giveUp(PATH);
    expect(sent).toEqual([{ type: 'release', path: '/channels/ch1/fader' }]);
  });
});

describe('selections', () => {
  it('lists the other sessions that selected an element, never this one', () => {
    const { s } = store();
    s.apply({
      type: 'patch',
      input: 0,
      changes: [],
      owners: [],
      selections: [
        { session: YOU, element: 'ch3' },
        { session: OTHER, element: 'ch3' },
        { session: NAMED, element: 'ch4' },
      ],
    });
    expect(s.selectedBy('ch3')).toEqual([OTHER]);
    expect(s.selectedBy('ch4')).toEqual([NAMED]);
    expect(s.selectedBy('ch5')).toEqual([]);
    expect(s.ownerInitial(NAMED)).toBe('D');
    expect(s.ownerInitial(OTHER)).toBe(handleTag(OTHER.handle).charAt(0));
    // The same handle gives the same initial on every screen.
    expect(store().s.ownerInitial({ ...OTHER, connection: 'c99' })).toBe(s.ownerInitial(OTHER));
  });

  it('forwards this screen selection to the worker', () => {
    const { s, sent } = store();
    s.select('ch2');
    s.select(null);
    expect(sent).toEqual([{ type: 'select', element: 'ch2' }, { type: 'select', element: null }]);
  });
});
