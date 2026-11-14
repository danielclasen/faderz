import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore } from '../../src/lib/state/mixer.svelte';
import { writesBlocked } from '../../src/lib/state/writes';
import type { SyncStatus, ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const replicas = (r1: boolean, r2: boolean, r3: boolean): SyncStatus['replicas'] => [
  { id: 'r1', role: 'leader', healthy: true, commitable: r1 },
  { id: 'r2', role: 'follower', healthy: true, commitable: r2 },
  { id: 'r3', role: 'follower', healthy: true, commitable: r3 },
];

const status = (extra: Partial<SyncStatus>): SyncStatus => ({ ...INITIAL_STATUS, link: 'Live', primaryReplica: 'r1', ...extra });

describe('writesBlocked', () => {
  it('blocks writes when the serving replica cannot commit and no standby is ready', () => {
    expect(writesBlocked(status({ replicas: replicas(false, false, false), standby: 'Idle' }))).toBe(true);
    expect(writesBlocked(status({ replicas: replicas(false, false, false), standby: 'Syncing' }))).toBe(true);
  });

  it('allows writes while a ready standby can take over', () => {
    expect(writesBlocked(status({ replicas: replicas(false, true, true), standby: 'Ready' }))).toBe(false);
  });

  it('allows writes when the serving replica can commit, whatever the others report', () => {
    expect(writesBlocked(status({ replicas: replicas(true, false, false), standby: 'Idle' }))).toBe(false);
  });

  it('does not block before the serving replica is known', () => {
    expect(writesBlocked(status({ replicas: [], standby: 'Idle' }))).toBe(false);
    expect(writesBlocked(status({ primaryReplica: null, replicas: replicas(false, false, false) }))).toBe(false);
  });
});

describe('mixer store without quorum', () => {
  function store() {
    const s = new MixerStore();
    const posted: ToWorker[] = [];
    s.attach({ post: (m) => posted.push(m) });
    const shape = exampleShape();
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [] });
    return { s, posted };
  }
  const lose = (s: MixerStore, commitable: boolean) =>
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status({ replicas: replicas(commitable, commitable, commitable) }) });

  it('neither applies nor sends writes and touches, and sends them again once quorum returns', () => {
    const { s, posted } = store();
    const path = '/channels/ch1/fader';
    const before = s.value<number>(path);
    lose(s, false);
    expect(s.locked).toBe(true);
    s.touch(path);
    s.set(path, before + 3);
    s.set('/channels/ch1/mute', true);
    expect(posted).toEqual([]);
    expect(s.value<number>(path)).toBe(before);
    expect(s.held.has(path)).toBe(false);
    lose(s, true);
    expect(s.locked).toBe(false);
    s.set(path, before + 3);
    expect(posted).toEqual([{ type: 'set', path, value: before + 3 }]);
  });

  it('still releases a touch made before quorum was lost', () => {
    const { s, posted } = store();
    const path = '/channels/ch1/fader';
    s.touch(path);
    lose(s, false);
    s.release(path);
    expect(posted).toEqual([{ type: 'touch', path }, { type: 'release', path }]);
  });
});
