import { TRANSMITTING } from '../sync/machines';
import type { SyncStatus } from '../sync/messages';

/**
 * Writes are blocked while the link transmits no commands (connecting,
 * offline, failing over, ahead of the cluster) and while the serving
 * replica reports that it cannot commit and no ready standby can take
 * over: a command written then goes unanswered or commits nowhere. With a
 * ready standby the worker fails over instead.
 */
export function writesBlocked(status: Pick<SyncStatus, 'link' | 'primaryReplica' | 'replicas' | 'standby'>): boolean {
  if (!TRANSMITTING.has(status.link)) return true;
  const serving = status.replicas.find((r) => r.id === status.primaryReplica);
  return serving !== undefined && !serving.commitable && status.standby !== 'Ready';
}
