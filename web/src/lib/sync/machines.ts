// Explicit state machines of the sync worker. Each is a transition table;
// an event a state does not list leaves the state unchanged.

export type Table<S extends string, E extends string> = Record<S, Partial<Record<E, S>>>;

export function step<S extends string, E extends string>(table: Table<S, E>, state: S, event: E): S {
  return table[state][event] ?? state;
}

// ---- primary connection ---------------------------------------------------

export type LinkState = 'Connecting' | 'Syncing' | 'Live' | 'Suspect' | 'FailingOver' | 'Resyncing' | 'Ahead' | 'Offline';
export type LinkEvent =
  /** socket opened, hello sent */
  | 'open'
  /** snapshot or resume applied */
  | 'synced'
  /** resync sent, after a revision gap or a meter layout mismatch */
  | 'resync'
  /** 1.5 heartbeat periods without traffic */
  | 'beatLate'
  /** traffic arrived */
  | 'beat'
  /** 3 heartbeat periods without traffic: the link is dead */
  | 'beatsMissed'
  /** socket closed or failed */
  | 'lost'
  /** the serving replica reports it cannot commit while a ready standby exists */
  | 'notCommitable'
  /** a ready standby was promoted to primary */
  | 'promoted'
  /** no ready standby to promote */
  | 'noStandby'
  /** consecutive connection attempts failed */
  | 'exhausted'
  /** backoff elapsed after Offline */
  | 'retry'
  /** the replica answered that this client holds a revision the cluster never committed */
  | 'ahead'
  /** the operator chose to take the cluster state */
  | 'adopt';

export const LINK_STATES: readonly LinkState[] = ['Connecting', 'Syncing', 'Live', 'Suspect', 'FailingOver', 'Resyncing', 'Ahead', 'Offline'];
export const LINK_EVENTS: readonly LinkEvent[] = [
  'open', 'synced', 'resync', 'beatLate', 'beat', 'beatsMissed', 'lost', 'notCommitable', 'promoted', 'noStandby', 'exhausted', 'retry', 'ahead', 'adopt',
];

export const LINK: Table<LinkState, LinkEvent> = {
  Connecting: { open: 'Syncing', exhausted: 'Offline' },
  Syncing: { synced: 'Live', lost: 'Connecting', beatsMissed: 'Connecting', ahead: 'Ahead' },
  // A replica that changes its epoch in place sends a snapshot of the new
  // one on an open connection: ahead from Live and Suspect too.
  Live: { resync: 'Resyncing', beatLate: 'Suspect', beatsMissed: 'FailingOver', lost: 'FailingOver', notCommitable: 'FailingOver', ahead: 'Ahead' },
  Suspect: { beat: 'Live', resync: 'Resyncing', beatsMissed: 'FailingOver', lost: 'FailingOver', notCommitable: 'FailingOver', ahead: 'Ahead' },
  FailingOver: { promoted: 'Resyncing', noStandby: 'Connecting' },
  Resyncing: { synced: 'Live', beatsMissed: 'FailingOver', lost: 'FailingOver', notCommitable: 'FailingOver', ahead: 'Ahead' },
  // Ahead waits for the operator; it never resyncs on its own, and a lost
  // link reconnects and is told again.
  Ahead: { adopt: 'Resyncing', beatsMissed: 'Connecting', lost: 'Connecting' },
  Offline: { retry: 'Connecting' },
};

/** States in which commands are transmitted on the primary connection. */
export const TRANSMITTING: ReadonlySet<LinkState> = new Set(['Live', 'Suspect', 'Resyncing']);

// ---- standby connection ---------------------------------------------------

export type StandbyState = 'Idle' | 'Connecting' | 'Syncing' | 'Ready';
export type StandbyEvent =
  | 'start'
  | 'open'
  | 'synced'
  /** landed on the primary's replica, or on one that cannot commit */
  | 'unsuitable'
  /** socket closed, failed or was closed by the client */
  | 'lost'
  | 'beatsMissed'
  /** handed over to become the primary */
  | 'promote';

export const STANDBY_STATES: readonly StandbyState[] = ['Idle', 'Connecting', 'Syncing', 'Ready'];
export const STANDBY_EVENTS: readonly StandbyEvent[] = ['start', 'open', 'synced', 'unsuitable', 'lost', 'beatsMissed', 'promote'];

export const STANDBY: Table<StandbyState, StandbyEvent> = {
  Idle: { start: 'Connecting' },
  Connecting: { open: 'Syncing', lost: 'Idle', beatsMissed: 'Idle' },
  Syncing: { synced: 'Ready', unsuitable: 'Idle', lost: 'Idle', beatsMissed: 'Idle' },
  Ready: { unsuitable: 'Idle', lost: 'Idle', beatsMissed: 'Idle', promote: 'Idle' },
};

// ---- command lifecycle ----------------------------------------------------

export type CommandState = 'Queued' | 'Sent' | 'Acked' | 'Rejected' | 'Stale' | 'Superseded' | 'Expired';
export type CommandEvent =
  | 'transmit'
  /** a delta listing its id, or ack noop or duplicate */
  | 'ack'
  /** error 400, 404 or 409 */
  | 'reject'
  /** ack stale */
  | 'stale'
  /** a newer value for the same continuous control replaced it before it was sent */
  | 'supersede'
  /** the connection it was sent on failed: it is sent again with the same id */
  | 'failover'
  /** no answer within the answer deadline: it is sent again with the same id */
  | 'timeout'
  /** error 503, not applied: it is sent again with the same id */
  | 'notApplied'
  /** a set still unanswered at its expiry: dropped, its control shows the committed value */
  | 'expire';

export const COMMAND_STATES: readonly CommandState[] = ['Queued', 'Sent', 'Acked', 'Rejected', 'Stale', 'Superseded', 'Expired'];
export const COMMAND_EVENTS: readonly CommandEvent[] = ['transmit', 'ack', 'reject', 'stale', 'supersede', 'failover', 'timeout', 'notApplied', 'expire'];

export const COMMAND: Table<CommandState, CommandEvent> = {
  Queued: { transmit: 'Sent', supersede: 'Superseded', ack: 'Acked', reject: 'Rejected', stale: 'Stale', expire: 'Expired' },
  Sent: { ack: 'Acked', reject: 'Rejected', stale: 'Stale', failover: 'Queued', timeout: 'Queued', notApplied: 'Queued', expire: 'Expired' },
  Acked: {},
  Rejected: {},
  Stale: {},
  Superseded: {},
  Expired: {},
};

export const TERMINAL: ReadonlySet<CommandState> = new Set(['Acked', 'Rejected', 'Stale', 'Superseded', 'Expired']);

// ---- touch ----------------------------------------------------------------

export type TouchState = 'Idle' | 'Touching' | 'Releasing';
export type TouchEvent =
  /** pointer down or keyboard adjust starts */
  | 'down'
  /** pointer up or keyboard adjust ends */
  | 'up'
  /** the release was answered */
  | 'released'
  /** the touch was refused (409) */
  | 'refused'
  /** the primary failed over; the state carries over and the worker touches again if needed */
  | 'failover';

export const TOUCH_STATES: readonly TouchState[] = ['Idle', 'Touching', 'Releasing'];
export const TOUCH_EVENTS: readonly TouchEvent[] = ['down', 'up', 'released', 'refused', 'failover'];

export const TOUCH: Table<TouchState, TouchEvent> = {
  Idle: { down: 'Touching' },
  Touching: { up: 'Releasing', refused: 'Idle' },
  Releasing: { released: 'Idle', down: 'Touching' },
};
