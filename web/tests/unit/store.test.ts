import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore } from '../../src/lib/state/mixer.svelte';
import type { Patch } from '../../src/lib/sync/messages';
import type { ToWorker } from '../../src/lib/sync/messages';
import type { Owner } from '../../src/lib/contract/types';
import { ownerHandle } from '../../src/lib/net/handle';
import { deriveMeterLayout } from '../../src/lib/contract/meterFrame';
import { exampleShape, initialState } from '../../dev/mock/shape';

/** Status of a synced link: writes are only taken while it transmits. */
const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

const F1 = '/channels/ch1/fader';
const SESSION = 'c2Vzc2lvbi1tZS0xMjM0NTY';
const ME: Owner = { principal: 'anonymous', handle: ownerHandle(SESSION), kind: 'ui', connection: 'c1' };

function store() {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
  return { s, posted };
}

const patch = (input: number, extra: Partial<Patch> = {}): Patch => ({ type: 'patch', input, changes: [], owners: [], ...extra });

describe('mixer store: patches', () => {
  it('writes one leaf and leaves its siblings and their parents in place', () => {
    const { s } = store();
    const state = s.state!;
    const ch1 = state.channels['ch1']!;
    const ch2 = state.channels['ch2']!;
    const eq = ch1.eq;
    s.apply(patch(0, { changes: [[F1, -12.5]] }));
    expect(s.state).toBe(state);
    expect(s.state!.channels['ch1']).toBe(ch1);
    expect(s.state!.channels['ch2']).toBe(ch2);
    expect(s.state!.channels['ch1']!.eq).toBe(eq);
    expect(ch1.fader).toBe(-12.5);
  });

  it('replaces the state only on a snapshot', () => {
    const { s } = store();
    const before = s.state;
    const shape = exampleShape();
    s.apply(patch(0, { snapshot: { shape, state: initialState(shape), owners: {} }, status: LIVE }));
    expect(s.state).not.toBe(before);
  });

  it('does not overwrite a local write with a patch built before the worker saw it', () => {
    const { s, posted } = store();
    s.set(F1, -3);
    expect(posted).toEqual([{ type: 'set', path: F1, value: -3 }]);
    expect(s.value(F1)).toBe(-3);
    s.apply(patch(0, { changes: [[F1, -40]] }));
    expect(s.value(F1)).toBe(-3);
    s.apply(patch(1, { changes: [[F1, -2]] }));
    expect(s.value(F1)).toBe(-2);
  });

  it('keeps a newer local write across a snapshot', () => {
    const { s } = store();
    s.set(F1, -3);
    const shape = exampleShape();
    s.apply(patch(0, { snapshot: { shape, state: initialState(shape), owners: {} }, status: LIVE }));
    expect(s.value(F1)).toBe(-3);
  });

  it('sets and deletes owners per control', () => {
    const { s } = store();
    s.apply(patch(0, { owners: [[F1, ME]] }));
    expect(s.ownerOf(F1)).toEqual(ME);
    s.apply(patch(0, { owners: [[F1, null]] }));
    expect(s.ownerOf(F1)).toBeNull();
  });

  it('measures each patch as faderz:patch', () => {
    const { s } = store();
    const n = s.patchStats.count;
    s.apply(patch(0, { changes: [[F1, 1]] }));
    expect(s.patchStats.count).toBe(n + 1);
    const m = performance.getEntriesByName('faderz:patch');
    expect(m).toHaveLength(1);
    expect(s.patchStats.last).toBe(m[0]!.duration);
  });

  it('notes the time of a new failover', () => {
    const { s } = store();
    expect(s.failoverAt).toBeNull();
    s.apply(patch(0, { status: { ...INITIAL_STATUS, failover: { n: 1, from: 'r1', to: 'r2', reason: 'dead' } } }));
    expect(s.failoverAt).not.toBeNull();
  });

  it('takes the latest meter frame with the replica that measured it', () => {
    const { s } = store();
    const layout = deriveMeterLayout(exampleShape());
    const values = new Float32Array(layout.valueCount).fill(-30);
    values[0] = -6;
    s.apply(patch(0, { meters: { replica: 'r2', sequence: 5, values } }), 100);
    expect(s.meterReplica).toBe('r2');
    expect(s.meters.get('channels', 'ch1')?.peak).toEqual([-6]);
    expect(s.meters.receivedAt).toBe(100);
  });
});

describe('mixer store: owners', () => {
  it('names the own session "you", a console by its name and another ui session by tag', () => {
    const { s } = store();
    s.apply(patch(0, { status: { ...INITIAL_STATUS, session: SESSION, handle: ME.handle, you: ME } }));
    const surface = s.state!.consoles[0]!;
    expect(s.ownerLabel({ ...ME, connection: 'c7' })).toBe('you');
    expect(s.ownerLabel({ principal: 'bridge', handle: surface.handle, kind: 'console', connection: 'x' })).toBe(surface.name);
    expect(s.ownerLabel({ principal: 'anonymous', handle: 'abcdef12-3456789012345A', kind: 'ui', connection: 'x' })).toBe('web client abcdef');
    expect(s.ownedByOther(F1)).toBe(false);
  });

  it('recognises its own ownership by the handle of its session before the server names it', () => {
    const { s } = store();
    s.apply(patch(0, { status: { ...INITIAL_STATUS, session: SESSION, handle: ownerHandle(SESSION), you: null } }));
    expect(s.isMine({ ...ME, connection: 'c3' })).toBe(true);
    expect(s.isMine({ ...ME, handle: ownerHandle('b3RoZXItc2Vzc2lvbi0xMjM') })).toBe(false);
  });

  it('shows a refused write with the owner name', () => {
    const { s } = store();
    s.apply(patch(0, { notices: [{ kind: 'conflict', path: F1, owner: { principal: 'anonymous', handle: 'abcdef12345678901234AB', kind: 'ui', connection: 'x' } }] }));
    expect(s.notices.at(-1)?.text).toBe('Pres 1 fader is held by web client abcdef');
    expect(s.conflicts.has(F1)).toBe(true);
  });
});

describe('notices', () => {
  it('keeps one notice per control path, counting repeats, and names a web client by kind and tag', () => {
    const s = new MixerStore();
    const owner: Owner = { principal: 'anonymous', handle: 'abcdefgh_ijklmnopqrstu', kind: 'ui', connection: 'c9' };
    const conflict = { kind: 'conflict' as const, path: '/channels/ch1/fader', owner };
    for (let i = 0; i < 5; i++) s.apply({ type: 'patch', input: 0, changes: [], owners: [], notices: [conflict] } as unknown as Patch);
    s.notify('error', 'something else');
    expect(s.notices).toHaveLength(2);
    expect(s.notices[0]).toMatchObject({ kind: 'conflict', count: 5, text: 'ch1 fader is held by web client abcdef' });
  });

  it('shows writes dropped after the failover bound as a notice, not an error: the controls show the committed values', () => {
    const { s } = store();
    s.apply(patch(0, { notices: [{ kind: 'expired', paths: [F1, '/channels/ch1/mute'] }] }));
    expect(s.notices).toHaveLength(1);
    expect(s.notices[0]).toMatchObject({ kind: 'info', text: 'Not confirmed in time, not sent again: Pres 1 fader, Pres 1 mute' });
  });
});
