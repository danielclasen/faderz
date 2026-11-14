import type { SyncStatus } from '../sync/messages';

/**
 * Writes are blocked while the serving replica reports that it cannot
 * commit and no ready standby can take over: a command sent then goes
 * unanswered or commits nowhere. With a ready standby the worker fails
 * over instead.
 */
export function writesBlocked(status: Pick<SyncStatus, 'primaryReplica' | 'replicas' | 'standby'>): boolean {
  const serving = status.replicas.find((r) => r.id === status.primaryReplica);
  return serving !== undefined && !serving.commitable && status.standby !== 'Ready';
}
