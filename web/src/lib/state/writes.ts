import { TRANSMITTING, type LinkState } from '../sync/machines';
import type { SyncStatus } from '../sync/messages';

/** Why writes are blocked in each link state that transmits no commands. */
const LINK_LOCK: Partial<Record<LinkState, string>> = {
  Connecting: 'reconnecting',
  Syncing: 'waiting for the replica to sync',
  FailingOver: 'failing over',
  Ahead: 'this client is ahead of the cluster',
  Offline: 'no replica reachable',
};

/**
 * Why writes are blocked, or null while they are not. They are blocked
 * while the link transmits no commands (connecting, offline, failing
 * over, ahead of the cluster) and while the serving replica reports that
 * it cannot commit and no ready standby can take over: a command written
 * then goes unanswered or commits nowhere. With a ready standby the
 * worker fails over instead.
 */
export function writesLockReason(status: Pick<SyncStatus, 'link' | 'primaryReplica' | 'replicas' | 'standby'>): string | null {
  if (!TRANSMITTING.has(status.link)) return LINK_LOCK[status.link] ?? status.link;
  const serving = status.replicas.find((r) => r.id === status.primaryReplica);
  return serving !== undefined && !serving.commitable && status.standby !== 'Ready' ? 'no quorum and no ready standby' : null;
}
