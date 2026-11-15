import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore } from '../../src/lib/state/mixer.svelte';
import { writesLockReason } from '../../src/lib/state/writes';
import { recallScene } from '../../src/lib/state/actions';
import type { SyncStatus, ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const replicas = (r1: boolean, r2: boolean, r3: boolean): SyncStatus['replicas'] => [
  { id: 'r1', role: 'leader', healthy: true, commitable: r1 },
  { id: 'r2', role: 'follower', healthy: true, commitable: r2 },
  { id: 'r3', role: 'follower', healthy: true, commitable: r3 },
];

const writesBlocked = (s: SyncStatus) => writesLockReason(s) !== null;
const status = (extra: Partial<SyncStatus>): SyncStatus => ({ ...INITIAL_STATUS, link: 'Live', primaryReplica: 'r1', ...extra });

describe('writesLockReason', () => {
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

  it('blocks writes whenever the link transmits no commands, whatever the replicas report', () => {
    const ok = { replicas: replicas(true, true, true), standby: 'Ready' as const };
    for (const link of ['Connecting', 'Syncing', 'FailingOver', 'Ahead', 'Offline'] as const) {
      expect(writesBlocked(status({ ...ok, link })), link).toBe(true);
      expect(writesBlocked(status({ ...ok, link, replicas: [], primaryReplica: null })), link).toBe(true);
    }
    for (const link of ['Live', 'Suspect', 'Resyncing'] as const) expect(writesBlocked(status({ ...ok, link })), link).toBe(false);
  });

  it('names why writes are blocked: the link state when it transmits nothing, else the quorum', () => {
    const ok = { replicas: replicas(true, true, true), standby: 'Ready' as const };
    const reasons = {
      Connecting: 'reconnecting',
      Syncing: 'waiting for the replica to sync',
      FailingOver: 'failing over',
      Ahead: 'this client is ahead of the cluster',
      Offline: 'no replica reachable',
    } as const;
    for (const [link, reason] of Object.entries(reasons) as [keyof typeof reasons, string][]) {
      expect(writesLockReason(status({ ...ok, link })), link).toBe(reason);
      // A link that transmits nothing is the reason even without quorum.
      expect(writesLockReason(status({ link, replicas: replicas(false, false, false), standby: 'Idle' })), link).toBe(reason);
    }
    expect(writesLockReason(status({ replicas: replicas(false, false, false), standby: 'Idle' }))).toBe('no quorum and no ready standby');
    expect(writesLockReason(status(ok))).toBeNull();
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
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: status({ replicas: replicas(true, true, true) }) });
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
    expect(s.lockReason).toBe('no quorum and no ready standby');
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

  it('sends no REST action while writes are off and names the lock reason in its notice', async () => {
    const { s } = store();
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status({ link: 'Offline', replicas: [], primaryReplica: null }) });
    expect(await recallScene(s, 's1', 0)).toBeNull();
    expect(s.notices.at(-1)).toMatchObject({ kind: 'error', text: 'Scene recall not sent: no replica reachable' });
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
