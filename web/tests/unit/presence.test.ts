import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore, REQUEST_INTERVAL_MS } from '../../src/lib/state/mixer.svelte';
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
  it('asks the holder of a control once per interval and says so', () => {
    const { s, sent } = store();
    expect(s.requestControl(PATH, 1000)).toBe(true);
    expect(sent).toEqual([{ type: 'request', path: PATH }]);
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
  });

  it('forwards this screen selection to the worker', () => {
    const { s, sent } = store();
    s.select('ch2');
    s.select(null);
    expect(sent).toEqual([{ type: 'select', element: 'ch2' }, { type: 'select', element: null }]);
  });
});
