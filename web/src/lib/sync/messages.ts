// Messages between the page and the sync worker. The page sends one message
// per user action and a tick per animation frame; the worker answers each
// tick with at most one patch.

import type { Owner, Owners, ReplicaStatus, Revision, Shape, State } from '../contract/types';
import type { LinkState, StandbyState } from './machines';

export type ToWorker =
  | { type: 'init'; endpoints: string[]; session: string; seqBase: number }
  | { type: 'set'; path: string; value: unknown }
  | { type: 'touch'; path: string }
  | { type: 'release'; path: string }
  /** Take the cluster state while the link is Ahead. */
  | { type: 'adopt' }
  /** The page rendered a frame and wants the changes since the last patch. */
  | { type: 'tick' };

export interface FailoverInfo {
  /** Counts failovers since the worker started. */
  n: number;
  from: string | null;
  to: string | null;
  reason: 'lost' | 'dead' | 'notCommitable';
}

export interface SyncStatus {
  link: LinkState;
  standby: StandbyState;
  session: string;
  you: Owner | null;
  primaryReplica: string | null;
  primaryEndpoint: string | null;
  standbyReplica: string | null;
  replicas: ReplicaStatus[];
  revision: Revision;
  /** Unanswered commands plus writes held by the rate limiter. */
  outbox: number;
  failover: FailoverInfo | null;
  /** Controls this session is touching, as the worker tracks them. */
  touching: string[];
  lastSync: 'snapshot' | 'resume' | null;
  meterCredits: number;
  /**
   * While the link is Ahead: this client's revision and the cluster's, and
   * whether the cluster is on another history (epoch), restored or started
   * empty, so the two revisions are not comparable.
   */
  ahead: { lastRevision: Revision; cluster: Revision; otherHistory: boolean } | null;
}

export type WorkerNotice =
  | { kind: 'conflict'; path: string; owner: Owner | null }
  | { kind: 'error'; text: string };

export interface Patch {
  type: 'patch';
  /** Page messages the worker had processed when it built this patch. */
  input: number;
  /** Replaces shape, state and owners; changes and owners below follow it. */
  snapshot?: { shape: Shape; state: State; owners: Owners };
  /** Changed leaves as [path, value], committed values or local values of pending writes. */
  changes: Array<[string, unknown]>;
  owners: Array<[string, Owner | null]>;
  status?: SyncStatus;
  /** Latest meter frame, flattened (meters/flat.ts); its buffer is transferred. */
  meters?: { replica: string; sequence: number; values: Float32Array };
  counters?: { replica: string; values: Record<string, number> };
  notices?: WorkerNotice[];
}

export type FromWorker = Patch;
