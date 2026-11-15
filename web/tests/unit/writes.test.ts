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
  it('blocks no write for a serving replica that cannot commit, with or without a standby: the operator can still try', () => {
    for (const standby of ['Idle', 'Syncing', 'Ready'] as const) {
      expect(writesBlocked(status({ replicas: replicas(false, false, false), standby })), standby).toBe(false);
      expect(writesBlocked(status({ replicas: replicas(false, true, true), standby })), standby).toBe(false);
    }
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
    expect(writesLockReason(status({ replicas: replicas(false, false, false), standby: 'Idle' }))).toBeNull();
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

  it('keeps every control usable and shows a key pressed meanwhile pending until its write is answered', () => {
    const { s, posted } = store();
    const mute = '/channels/ch1/mute';
    expect(s.notCommitting).toBe(false);
    lose(s, false);
    expect(s.notCommitting).toBe(true);
    expect(s.locked).toBe(false);
    s.set(mute, true);
    expect(posted).toEqual([{ type: 'set', path: mute, value: true }]);
    expect(s.value<boolean>(mute)).toBe(true);
    const report = (pendingKeys: string[], commitable = false) =>
      s.apply({ type: 'patch', input: 1, changes: [], owners: [], status: status({ replicas: replicas(commitable, commitable, commitable), pendingKeys }) });
    report([mute]);
    expect(s.pendingKey(mute)).toBe(true);
    expect(s.pendingKey('/channels/ch2/mute')).toBe(false);
    // Committing again, a write on its way is not marked: it is the normal round trip.
    report([mute], true);
    expect(s.pendingKey(mute)).toBe(false);
    report([], false);
    expect(s.pendingKey(mute)).toBe(false);
  });

  it('sends no REST action while writes are off and names the lock reason in its notice', async () => {
    const { s } = store();
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status({ link: 'Offline', replicas: [], primaryReplica: null }) });
    expect(await recallScene(s, 's1', 0)).toBeNull();
    expect(s.notices.at(-1)).toMatchObject({ kind: 'error', text: 'Scene recall not sent: no replica reachable' });
  });

  it('still touches and releases while quorum is lost', () => {
    const { s, posted } = store();
    const path = '/channels/ch1/fader';
    s.touch(path);
    lose(s, false);
    s.release(path);
    s.touch(path);
    expect(posted).toEqual([{ type: 'touch', path }, { type: 'release', path }, { type: 'touch', path }]);
  });
});
