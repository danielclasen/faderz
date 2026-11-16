import { TRANSMITTING, type LinkState } from '../sync/machines';
import type { SyncStatus } from '../sync/messages';

/** Why writes are blocked in each link state that transmits no commands. */
const LINK_LOCK: Partial<Record<LinkState, string>> = {
  Connecting: 'connecting',
  Syncing: 'waiting for the replica to sync',
  FailingOver: 'failing over',
  Ahead: 'this client is ahead of the cluster',
  Offline: 'no replica reachable',
};

/**
 * Why writes are blocked, or null while they are not: while the link
 * transmits no commands (connecting, offline, failing over, ahead of the
 * cluster). A serving replica that cannot commit blocks nothing: the
 * operator can still try, a write is answered not applied and sent again
 * until it commits or expires, and the screen shows it pending meanwhile
 * (MixerStore.pendingKey, CommitBanner).
 */
export function writesLockReason(status: Pick<SyncStatus, 'link'>): string | null {
  return TRANSMITTING.has(status.link) ? null : (LINK_LOCK[status.link] ?? status.link);
}
