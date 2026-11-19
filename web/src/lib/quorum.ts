// What the replica set's reports say about its quorum, for the status
// area: how many replicas can commit, how many a quorum needs, which one
// leads, and whether the replica serving this screen can commit.

import type { ReplicaStatus } from './contract/types';

export interface Quorum {
  total: number;
  /** A majority of the set. */
  needed: number;
  commitableCount: number;
  leader: string | null;
  /** Whether the serving replica can commit; null before it has reported. */
  commitable: boolean | null;
}

export function quorumOf(replicas: readonly ReplicaStatus[], serving: string | null): Quorum {
  const total = replicas.length;
  const self = replicas.find((r) => r.id === serving);
  return {
    total,
    needed: Math.floor(total / 2) + 1,
    commitableCount: replicas.filter((r) => r.commitable).length,
    leader: replicas.find((r) => r.role === 'leader')?.id ?? null,
    commitable: self ? self.commitable : null,
  };
}
