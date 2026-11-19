// Sync engine of the web client, run by the sync worker. Keeps a primary
// WebSocket and a standby one to another replica, heartbeats both, detects
// a dead or non-committing primary and promotes the standby, holds the
// replicated state, the command outbox and the meter credits, and builds
// one patch per page tick. Pure: sockets, clock and posting are injected,
// so it runs in a worker and under a fake clock in tests.

import { decodeMeterFrame, deriveMeterLayout, MeterFrameError, type MeterLayout } from '../contract/meterFrame';
import { COMMAND_EXPIRY_MS, DEAD_MS, HEARTBEAT_MS, MAX_COMMANDS_IN_FLIGHT, METER_CREDIT_CAP, SUSPECT_MS } from '../contract/protocol';
import type {
  ClientMessage,
  CommandMessage,
  DeltaMessage,
  Epoch,
  ErrorMessage,
  Owner,
  Owners,
  ReplicaRef,
  Build,
  ReplicaStatus,
  PresentSession,
  Selection,
  AheadMessage,
  ResumeMessage,
  ServerMessage,
  Shape,
  SnapshotMessage,
  State,
} from '../contract/types';
import { flattenMeterFrame } from '../meters/flat';
import { ownerHandle } from '../net/handle';
import { uuid } from '../net/ids';
import { getAt, setAt } from '../state/pointer';
import type { Clock } from './clock';
import { RateLimiter } from './limiter';
import {
  LINK,
  STANDBY,
  TOUCH,
  TRANSMITTING,
  step,
  type LinkEvent,
  type LinkState,
  type StandbyEvent,
  type StandbyState,
  type TouchEvent,
  type TouchState,
} from './machines';
import type { FailoverInfo, Patch, SyncStatus, ToWorker, WorkerNotice } from './messages';
import { withSession } from './session';
import { isContinuousPath } from '../contract/continuous';
import { Outbox } from './outbox';
import { backoff, RttEstimator } from './rtt';

export interface SocketLike {
  binaryType: string;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export interface EngineDeps {
  endpoints: readonly string[];
  session: string;
  /** Unix time in ms at page load; every control's seq starts here. */
  seqBase: number;
  socket(url: string): SocketLike;
  clock: Clock;
  post(patch: Patch, transfer: Transferable[]): void;
  /** Uniform in [0, 1), for retry jitter; Math.random when absent. */
  random?: () => number;
}

// Deadlines follow the link: each is a floor that holds on a LAN, raised
// by the measured round-trip time (rtt.ts) and doubled per consecutive
// failure, up to a cap. A socket still opening is never abandoned at a
// LAN-sized deadline on a link that needs longer; a socket the peer
// refuses or closes fails over at once, whatever the deadlines.

/** Floor of the time a socket may take to open. */
export const CONNECT_TIMEOUT_MS = 2000;
/** The time a socket may take to open while nothing is known about the link. */
export const FIRST_CONNECT_TIMEOUT_MS = 10_000;
/** An open takes TCP, TLS and the upgrade: about three round trips, and one spare. */
export const CONNECT_ROUND_TRIPS = 4;
export const CONNECT_TIMEOUT_CAP_MS = 30_000;
/** Delay before the next primary attempt, by consecutive failures. */
export const PRIMARY_RETRY_MS: readonly number[] = [0, 250, 500];
/** Consecutive failed primary attempts that make the link Offline. */
export const PRIMARY_ATTEMPTS = 3;
/** Delay before leaving Offline, doubled per Offline in a row up to the cap. */
export const OFFLINE_RETRY_MS = 2000;
export const RETRY_CAP_MS = 15_000;
/** Failed standby attempts in a row, misses (STANDBY_MISSES) aside, before the standby backs off. */
export const STANDBY_TRIES = 4;
export const STANDBY_RETRY_MS = 100;
/** First standby backoff, doubled per further failed attempt up to RETRY_CAP_MS. */
export const STANDBY_BACKOFF_MS = 2000;
/**
 * Standbys in a row that landed on the primary's replica while a report
 * named another healthy one, retried after STANDBY_RETRY_MS without
 * counting as failed attempts. An endpoint in front of several replicas
 * routes each connection on its own: with three, 16 landings in a row on
 * one of them happen once in 43 million. The bound keeps an endpoint that
 * pins the session to one replica from being asked ten times a second.
 */
export const STANDBY_MISSES = 16;
/** Delay before asking again after an answer older than the client's revision. */
export const RESYNC_RETRY_MS = 250;
/**
 * Floor of the time a primary has to answer a hello, resync or promote
 * with state this client can apply; one that has not by then is left for
 * another endpoint. Above the 2 s a replica holds an answer for its read
 * index (READ_INDEX_HOLD_MS).
 */
export const SYNC_TIMEOUT_MS = 3000;
export const READ_INDEX_HOLD_MS = 2000;
export const SYNC_TIMEOUT_CAP_MS = 30_000;
/**
 * Most the heartbeat watch and the commit settle are lengthened by on a
 * slow link: the server closes a silent connection within 7 s.
 */
export const SLACK_CAP_MS = 5000;
/** A command answered not applied is sent again with its id after this. */
export const NOT_APPLIED_RETRY_MS = 250;
/**
 * A primary that reports it cannot commit is left for the standby only
 * if the standby still reports, this long later, that it can: a quorum
 * lost by every replica reaches the two connections at slightly
 * different times, and a standby whose own report is still on its way
 * would otherwise be promoted onto a replica that cannot commit either.
 */
export const COMMIT_SETTLE_MS = HEARTBEAT_MS;
/** Credits are topped up to the cap once this many or fewer are outstanding. */
export const CREDIT_REFILL_AT = METER_CREDIT_CAP / 2;
const NOTICE_LIMIT = 20;

type Role = 'primary' | 'standby';

class Link {
  replica: ReplicaRef | null = null;
  replicas: ReplicaStatus[] | null = null;
  build: Build | null = null;
  /** The latest selections this connection reported (Selections); only the primary's are shown. */
  selections: Selection[] | null = null;
  /** The sessions connected to this connection's replica (Presence); only the primary's are shown. */
  presence: PresentSession[] | null = null;
  opened = false;
  suspect = false;
  lastRx: number;
  readonly createdAt: number;
  cancelWatch: (() => void) | null = null;
  /** Until it opens: the time it is abandoned at if still opening. */
  connectBy = Infinity;
  /** t of the pings sent and not answered yet, oldest first. */
  readonly pings: number[] = [];

  constructor(
    public role: Role,
    readonly endpoint: number,
    readonly socket: SocketLike,
    now: number,
  ) {
    this.lastRx = now;
    this.createdAt = now;
  }
}

export class SyncEngine {
  link: LinkState = 'Connecting';
  standbyState: StandbyState = 'Idle';
  primary: Link | null = null;
  standby: Link | null = null;
  state: State | null = null;
  shape: Shape | null = null;
  owners: Owners = {};
  revision = 0;
  /** Epoch of revision, set with the state by the first snapshot. */
  epoch: Epoch | null = null;
  you: Owner | null = null;
  /** Meter credits outstanding on the primary connection. */
  credits = 0;
  readonly outbox = new Outbox();
  readonly touches = new Map<string, TouchState>();
  /** The element selected on this screen, sent again on every new primary. */
  private selection: string | null = null;
  private selectionsDirty = false;
  /** Whether a command (a set, touch or release) came in since the last patch; the next patch then goes out. */
  private commanded = false;
  private presenceDirty = false;

  private readonly clock: Clock;
  private readonly random: () => number;
  /** Owner handle of this session: how its ownership appears in records. */
  private readonly handle: string;
  private readonly limiter: RateLimiter;
  private readonly seq = new Map<string, number>();
  /** Latest local value of every control with a write not yet answered. */
  private readonly localValue = new Map<string, unknown>();
  private readonly dirty = new Set<string>();
  private readonly ownersDirty = new Set<string>();
  private snapshotPending = false;
  /** Set by adopt(): the next snapshot replaces the state even if it is older. */
  private adopting = false;
  private ahead: SyncStatus['ahead'] = null;
  private notices: WorkerNotice[] = [];
  private meters: Patch['meters'] | null = null;
  private counters: Patch['counters'] | null = null;
  private lastStatus = '';
  private layout: MeterLayout | null = null;
  private layoutResyncSent = false;
  private primaryIndex = 0;
  private primaryAttempts = 0;
  private standbyAttempts = 0;
  /** Standby attempts since the last synced standby that failed; misses do not count. */
  private standbyFailures = 0;
  /** Standbys in a row that landed on the primary's replica by chance (STANDBY_MISSES); a failure ends the run. */
  private standbyMisses = 0;
  /** Offline periods in a row, since the last sync. */
  private offlineRounds = 0;
  /**
   * Sockets in a row abandoned before they opened, per role: a standby
   * endpoint that never answers lengthens no primary deadline.
   */
  private readonly connectTimeouts: Record<Role, number> = { primary: 0, standby: 0 };
  /** Primaries in a row left for giving no state in time. */
  private syncTimeouts = 0;
  /** Round trips of pings, and the page's own request to the server. */
  readonly rtt = new RttEstimator();
  /** How long sockets took from creation to open. */
  readonly opening = new RttEstimator();
  private lastPrimaryReplica: string | null = null;
  private failoverInfo: FailoverInfo | null = null;
  /** Hellos and resyncs sent on primaries: the generation a command is sent in. */
  private syncRequests = 0;
  private lastSync: SyncStatus['lastSync'] = null;
  private inputs = 0;
  private cancelRetry: (() => void) | null = null;
  private cancelStandbyRetry: (() => void) | null = null;
  private cancelResyncRetry: (() => void) | null = null;
  private cancelSyncTimeout: (() => void) | null = null;
  private cancelCommitSettle: (() => void) | null = null;

  constructor(private readonly deps: EngineDeps) {
    if (deps.endpoints.length === 0) throw new Error('no endpoints');
    this.clock = deps.clock;
    this.random = deps.random ?? Math.random;
    this.handle = ownerHandle(deps.session);
    this.limiter = new RateLimiter(this.clock, (path, value) => this.issueSet(path, value));
  }

  // ---- timing ----------------------------------------------------------------

  /**
   * Round-trip time the deadlines assume: the ping estimate, else a third
   * of the open estimate (an open is about three round trips, so this
   * bounds one from above), else null.
   */
  private roundTrip(): number | null {
    const r = this.rtt.rto;
    if (r !== null) return r;
    const o = this.opening.rto;
    return o === null ? null : o / 3;
  }

  /**
   * How long a new socket may take to open: FIRST_CONNECT_TIMEOUT_MS while
   * nothing is known about the link, else the larger of the floor,
   * CONNECT_ROUND_TRIPS round trips and the open estimate; doubled per
   * socket of `role` in a row abandoned unopened, up to the cap.
   */
  connectTimeout(role: Role): number {
    const rt = this.rtt.rto;
    const open = this.opening.rto;
    const base =
      rt === null && open === null
        ? FIRST_CONNECT_TIMEOUT_MS
        : Math.max(CONNECT_TIMEOUT_MS, CONNECT_ROUND_TRIPS * (rt ?? 0), open ?? 0);
    return backoff(base, this.connectTimeouts[role], CONNECT_TIMEOUT_CAP_MS);
  }

  /**
   * How long a primary has to answer with state: the read-index hold plus
   * two round trips, at least SYNC_TIMEOUT_MS; doubled per primary in a
   * row left for not answering, up to the cap.
   */
  syncTimeout(): number {
    const base = Math.max(SYNC_TIMEOUT_MS, READ_INDEX_HOLD_MS + 2 * (this.roundTrip() ?? 0));
    return backoff(base, this.syncTimeouts, SYNC_TIMEOUT_CAP_MS);
  }

  /**
   * What the heartbeat watch and the commit settle add on this link: one
   * round-trip timeout. A loss on a slow TCP path holds every later
   * message back for about that long, so a silence that short is the link,
   * not a dead peer. Nothing on a LAN, where it is a few milliseconds.
   */
  private slack(): number {
    // Whole milliseconds: a fractional deadline can round to just short of
    // itself, and a watch woken short of it re-arms for no time at all.
    return Math.ceil(Math.min(SLACK_CAP_MS, this.rtt.rto ?? 0));
  }

  /** Silence after which a connection is suspect. */
  suspectMs(): number {
    return SUSPECT_MS + this.slack();
  }

  /** Silence after which a connection is dead. */
  deadMs(): number {
    return DEAD_MS + this.slack();
  }

  start(): void {
    this.openPrimary();
    const beat = () => {
      const t = Math.max(0, Math.floor(this.clock.now()));
      for (const l of [this.primary, this.standby]) {
        if (!l?.opened) continue;
        this.send(l, { type: 'ping', t });
        l.pings.push(t);
        // Pings a dead peer never answers: the watch closes it long before.
        if (l.pings.length > 64) l.pings.shift();
      }
      this.sweep();
      this.deadline(beat, HEARTBEAT_MS);
    };
    this.deadline(beat, HEARTBEAT_MS);
  }

  /**
   * Runs fn `ms` from now. A timer that fires more than SUSPECT_MS late
   * shows that the worker itself stalled (garbage collection, a busy CPU),
   * and the messages received meanwhile may still wait behind it: fn then
   * yields to them once, from a 0 ms timer, before it judges what did not
   * arrive. Returns the cancel of whichever timer is pending.
   */
  private deadline(fn: () => void, ms: number): () => void {
    const at = this.clock.now() + Math.max(0, ms);
    let cancel = this.clock.schedule(() => {
      if (this.clock.now() - at > SUSPECT_MS) cancel = this.clock.schedule(fn, 0);
      else fn();
    }, ms);
    return () => cancel();
  }

  input(msg: ToWorker): void {
    // A round trip the page measured is not a page input a patch answers.
    if (msg.type === 'rtt') {
      this.rtt.sample(msg.ms);
      // A socket opened before anything was measured waits no longer
      // than the measurement allows.
      for (const l of [this.primary, this.standby]) if (l && !l.opened) this.armConnect(l);
      return;
    }
    this.inputs++;
    if (msg.type === 'set' || msg.type === 'touch' || msg.type === 'release') this.commanded = true;
    switch (msg.type) {
      case 'set':
        this.localValue.set(msg.path, msg.value);
        if (isContinuousPath(msg.path)) this.limiter.write(msg.path, msg.value);
        else this.issueSet(msg.path, msg.value);
        return;
      case 'touch':
        return this.touchEvent(msg.path, 'down');
      case 'release':
        return this.touchEvent(msg.path, 'up');
      case 'select':
        if (msg.element === this.selection) return;
        this.selection = msg.element;
        return this.sendSelection();
      case 'request': {
        const p = this.primary;
        if (!p || !TRANSMITTING.has(this.link)) return;
        this.send(p, { type: 'request', path: msg.path });
        this.notice({ kind: 'requested', path: msg.path, owner: this.owners[msg.path] ?? null });
        return;
      }
      case 'tick':
        this.grantCredits();
        return this.postPatch();
      case 'adopt':
        return this.adopt();
      case 'init':
        return;
    }
  }

  // ---- commands -------------------------------------------------------------

  /** Next seq of a control, on the counter that its sets, touches and releases share. */
  private nextSeq(path: string): number {
    const seq = (this.seq.get(path) ?? this.deps.seqBase) + 1;
    this.seq.set(path, seq);
    return seq;
  }

  private sendSelection(): void {
    const p = this.primary;
    if (p && TRANSMITTING.has(this.link)) this.send(p, { type: 'select', element: this.selection });
  }

  private issueSet(path: string, value: unknown): void {
    this.issue({ type: 'set', id: uuid(), seq: this.nextSeq(path), path, value });
  }

  private issueOwnership(type: 'touch' | 'release', path: string): void {
    this.issue({ type, id: uuid(), seq: this.nextSeq(path), path });
  }

  private issue(msg: CommandMessage): void {
    const r = this.outbox.enqueue(msg, msg.type === 'set' && isContinuousPath(msg.path), this.clock.now());
    if (!r.accepted) {
      this.notice({ kind: 'error', text: `Too many unanswered commands; ${msg.type} of ${msg.path} was not sent` });
      this.settled(msg);
      return;
    }
    this.transmit();
  }

  private transmit(): void {
    const p = this.primary;
    if (!p?.opened || !TRANSMITTING.has(this.link)) return;
    // A command past its expiry between two sweeps is left for the next
    // one to drop: the contract has no command sent more than
    // COMMAND_EXPIRY_MS after it was issued.
    const now = this.clock.now();
    for (const m of this.outbox.transmit(now, MAX_COMMANDS_IN_FLIGHT, now - COMMAND_EXPIRY_MS, this.syncRequests)) this.send(p, m);
  }

  private answer(id: string, event: 'ack' | 'reject' | 'stale'): void {
    const e = this.outbox.answer(id, event);
    if (!e) return;
    const m = e.msg;
    if (m.type === 'touch' && event === 'reject') this.touchStep(m.path, 'refused');
    if (m.type === 'release' && !this.outbox.has('release', m.path)) this.touchStep(m.path, 'released');
    this.settled(m);
    // A touch or release refused as stale changed nothing: a later command
    // of this control decided its ownership, which may not be what the
    // touch wants now.
    if (m.type !== 'set' && event === 'stale') this.reconcile(m.path);
    this.transmit();
  }

  /**
   * Once per heartbeat: a command unanswered COMMAND_EXPIRY_MS after it was
   * issued is dropped, so no command pins its control for longer. No
   * deadline sends one again: the server answers every command it reads,
   * and a sent command stays in flight until answered or dropped.
   */
  private sweep(): void {
    const expired = this.outbox.expire(this.clock.now() - COMMAND_EXPIRY_MS);
    const paths = new Set<string>();
    for (const { msg } of expired) {
      if (msg.type === 'set') paths.add(msg.path);
      if (msg.type === 'release' && !this.outbox.has('release', msg.path)) this.touchStep(msg.path, 'released');
      this.settled(msg);
    }
    if (paths.size > 0) this.notice({ kind: 'expired', paths: [...paths] });
    // Ownership the dropped touches and releases left unsettled is put right with new commands.
    if (TRANSMITTING.has(this.link)) for (const { msg } of expired) if (msg.type !== 'set') this.reconcile(msg.path);
    this.transmit();
  }

  /** A set left the outbox: once nothing is pending for its control, the committed value goes out. */
  private settled(m: CommandMessage): void {
    if (m.type !== 'set' || this.pending(m.path)) return;
    this.localValue.delete(m.path);
    this.dirty.add(m.path);
  }

  private pending(path: string): boolean {
    return this.limiter.holds(path) || this.outbox.hasSet(path);
  }

  /**
   * Ownership of a control as this session's touch wants it: released
   * while it is not touching, touched again while it is.
   */
  private reconcile(path: string): void {
    const touching = this.touches.get(path) === 'Touching';
    const mine = this.mine(this.owners[path]);
    if (mine && !touching && !this.outbox.has('release', path)) this.issueOwnership('release', path);
    if (!mine && touching && !this.outbox.has('touch', path)) this.issueOwnership('touch', path);
  }

  private touchEvent(path: string, event: 'down' | 'up'): void {
    const prev = this.touches.get(path) ?? 'Idle';
    if (this.touchStep(path, event) === prev) return;
    this.issueOwnership(event === 'down' ? 'touch' : 'release', path);
  }

  private touchStep(path: string, event: TouchEvent): TouchState {
    const next = step(TOUCH, this.touches.get(path) ?? 'Idle', event);
    if (next === 'Idle') this.touches.delete(path);
    else this.touches.set(path, next);
    return next;
  }

  private mine(owner: Owner | null | undefined): boolean {
    if (!owner) return false;
    if (this.you) return owner.principal === this.you.principal && owner.handle === this.you.handle;
    return owner.handle === this.handle;
  }

  // ---- connections -----------------------------------------------------------

  private connect(role: Role, index: number): Link {
    const socket = this.deps.socket(withSession(this.deps.endpoints[index]!, this.deps.session));
    socket.binaryType = 'arraybuffer';
    const link = new Link(role, index, socket, this.clock.now());
    this.armConnect(link);
    socket.onopen = () => this.opened(link);
    socket.onmessage = (ev) => this.message(link, ev.data);
    socket.onclose = () => this.closed(link, 'lost');
    socket.onerror = () => {
      // onclose follows and handles the failure.
    };
    return link;
  }

  /**
   * Arms the connect deadline of a link still opening, or moves it
   * earlier when connectTimeout() has shrunk since; never later. Only a
   * socket still opening at its deadline is abandoned here; one the peer
   * refuses or closes fails through onclose at once.
   */
  private armConnect(link: Link): void {
    const by = link.createdAt + this.connectTimeout(link.role);
    if (by >= link.connectBy) return;
    link.connectBy = by;
    link.cancelWatch?.();
    link.cancelWatch = this.deadline(() => {
      if (link !== this.primary && link !== this.standby) return;
      this.connectTimeouts[link.role]++;
      this.closed(link, 'lost');
    }, by - this.clock.now());
  }

  private detach(link: Link): void {
    const s = link.socket;
    s.onopen = s.onmessage = s.onclose = s.onerror = null;
    link.cancelWatch?.();
    link.cancelWatch = null;
    link.opened = false;
    s.close(4000, 'closed by client');
  }

  private send(link: Link, msg: ClientMessage): void {
    if (!link.opened) return;
    if (link === this.primary && (msg.type === 'hello' || msg.type === 'resync')) this.syncRequests++;
    link.socket.send(JSON.stringify(msg));
  }

  private opened(link: Link): void {
    link.opened = true;
    link.lastRx = this.clock.now();
    this.opening.sample(link.lastRx - link.createdAt);
    this.connectTimeouts[link.role] = 0;
    this.watch(link);
    if (link.role === 'primary') {
      this.credits = 0;
      this.linkEvent('open');
      this.awaitSync();
      this.send(link, {
        type: 'hello',
        session: this.deps.session,
        kind: 'ui',
        mode: 'primary',
        ...this.position(),
      });
    } else {
      this.standbyEvent('open');
      this.send(link, { type: 'hello', session: this.deps.session, kind: 'ui', mode: 'standby', ...this.position() });
    }
  }

  /** Where this client is in the cluster history; empty before the first snapshot. */
  private position(): { lastRevision: number; epoch: Epoch } | Record<string, never> {
    return this.state && this.epoch !== null ? { lastRevision: this.revision, epoch: this.epoch } : {};
  }

  /**
   * Re-arms the traffic watch of a link: suspect after suspectMs(), dead
   * after deadMs(), SUSPECT_MS and DEAD_MS on a LAN. A late watch yields
   * to the messages a stall held up before it counts the silence
   * (deadline()).
   */
  private watch(link: Link): void {
    link.cancelWatch?.();
    const due = Math.ceil(link.lastRx + (link.suspect ? this.deadMs() : this.suspectMs()) - this.clock.now());
    link.cancelWatch = this.deadline(() => {
      const silence = this.clock.now() - link.lastRx;
      if (silence >= this.deadMs()) return this.closed(link, 'dead');
      if (silence >= this.suspectMs() && !link.suspect) {
        link.suspect = true;
        if (link === this.primary) this.linkEvent('beatLate');
      }
      this.watch(link);
    }, due);
  }

  private closed(link: Link, why: 'lost' | 'dead' | 'noAnswer'): void {
    if (link !== this.primary && link !== this.standby) return;
    this.detach(link);
    if (link === this.standby) {
      this.standby = null;
      this.standbyFailed();
      this.standbyEvent(why === 'dead' ? 'beatsMissed' : 'lost');
      this.scheduleStandby();
      return;
    }
    if (TRANSMITTING.has(this.link)) return this.failover(why);
    this.primary = null;
    this.linkEvent(why === 'dead' ? 'beatsMissed' : 'lost');
    this.retryPrimary();
  }

  private openPrimary(): void {
    this.cancelRetry = null;
    this.primary = this.connect('primary', this.primaryIndex);
  }

  private retryPrimary(): void {
    this.primaryAttempts++;
    this.primaryIndex = (this.primaryIndex + 1) % this.deps.endpoints.length;
    this.cancelRetry?.();
    if (this.primaryAttempts >= PRIMARY_ATTEMPTS) {
      this.linkEvent('exhausted');
      const delay = backoff(OFFLINE_RETRY_MS, this.offlineRounds++, RETRY_CAP_MS, this.random);
      this.cancelRetry = this.clock.schedule(() => {
        this.primaryAttempts = 0;
        this.linkEvent('retry');
        this.openPrimary();
      }, delay);
      return;
    }
    this.cancelRetry = this.clock.schedule(() => this.openPrimary(), PRIMARY_RETRY_MS[this.primaryAttempts] ?? 0);
  }

  /**
   * Leaves the primary when it has given no state this client can apply
   * syncTimeout() after now: a replica that holds its answer longer, or
   * answers only with older state, is passed over.
   */
  private awaitSync(): void {
    this.cancelSyncTimeout?.();
    const p = this.primary;
    this.cancelSyncTimeout = this.deadline(() => {
      this.cancelSyncTimeout = null;
      if (p === this.primary && p && (this.link === 'Syncing' || this.link === 'Resyncing')) {
        this.syncTimeouts++;
        this.closed(p, 'noAnswer');
      }
    }, this.syncTimeout());
  }

  private failover(reason: FailoverInfo['reason']): void {
    const old = this.primary;
    const from = old?.replica?.id ?? this.lastPrimaryReplica;
    this.linkEvent(reason === 'notCommitable' ? 'notCommitable' : reason === 'dead' ? 'beatsMissed' : 'lost');
    if (old) {
      if (old.opened) this.detach(old);
      this.primary = null;
    }
    this.outbox.failover();
    for (const path of [...this.touches.keys()]) this.touchStep(path, 'failover');
    this.credits = 0;
    this.meters = null;
    const sb = this.standby;
    const n = (this.failoverInfo?.n ?? 0) + 1;
    this.cancelCommitSettle?.();
    this.cancelCommitSettle = null;
    if (sb && this.standbyState === 'Ready') {
      this.standbyEvent('promote');
      this.standby = null;
      sb.role = 'primary';
      this.primary = sb;
      this.selectionsDirty = true;
      this.presenceDirty = true;
      this.primaryIndex = sb.endpoint;
      this.primaryAttempts = 0;
      this.failoverInfo = { n, from, to: sb.replica?.id ?? null, reason };
      // Failover happens only after a snapshot, so the state and epoch are set.
      this.send(sb, { type: 'promote', lastRevision: this.revision, epoch: this.epoch! });
      this.linkEvent('promoted');
      this.transmit();
      this.resync(false);
      this.startStandby();
      return;
    }
    if (sb) {
      this.detach(sb);
      this.standby = null;
      this.standbyEvent('lost');
    }
    this.failoverInfo = { n, from, to: null, reason };
    this.linkEvent('noStandby');
    this.retryPrimary();
  }

  private scheduleStandby(): void {
    if (this.standbyState !== 'Idle' || this.cancelStandbyRetry) return;
    const failures = this.standbyFailures;
    const delay =
      failures < STANDBY_TRIES ? STANDBY_RETRY_MS : backoff(STANDBY_BACKOFF_MS, failures - STANDBY_TRIES, RETRY_CAP_MS, this.random);
    this.cancelStandbyRetry = this.clock.schedule(() => {
      this.cancelStandbyRetry = null;
      this.startStandby();
    }, delay);
  }

  private startStandby(): void {
    if (this.standbyState !== 'Idle' || !this.primary || !this.state || !TRANSMITTING.has(this.link)) return;
    const n = this.deps.endpoints.length;
    const others = Array.from({ length: n }, (_, i) => i).filter((i) => i !== this.primaryIndex);
    const index = others.length > 0 ? others[this.standbyAttempts % others.length]! : 0;
    this.standbyAttempts++;
    this.standbyEvent('start');
    this.standby = this.connect('standby', index);
  }

  /** Whether the replica of a link reports that it can commit; undefined before it has reported. */
  private commitable(link: Link | null): boolean | undefined {
    const id = link?.replica?.id;
    return id ? link.replicas?.find((r) => r.id === id)?.commitable : undefined;
  }

  /** A standby on another replica than the primary's; whether it can commit is checkCommitable's matter. */
  private standbySuitable(link: Link): boolean {
    const id = link.replica?.id;
    return !!id && id !== this.primary?.replica?.id;
  }

  /** A ready standby whose replica reports that it can commit. */
  private promotable(): boolean {
    return this.standbyState === 'Ready' && this.commitable(this.standby) === true;
  }

  /**
   * Lets go of a standby on the primary's replica. When a report names
   * another healthy replica, the endpoint could have routed it there: a
   * miss, up to STANDBY_MISSES in a row, not a failed attempt.
   */
  private missStandby(link: Link): void {
    const reported = link.replicas ?? this.primary?.replicas ?? [];
    if (this.standbyMisses < STANDBY_MISSES && reported.some((r) => r.healthy && r.id !== link.replica?.id)) {
      this.standbyMisses++;
    } else {
      // Past the misses, or with no other replica to land on, a landing
      // on the primary's replica fails without ending the run of them.
      this.standbyFailures++;
    }
    this.letGoStandby(link);
  }

  /** A standby attempt that failed other than by landing on the primary's replica, which ends a run of misses. */
  private standbyFailed(): void {
    this.standbyFailures++;
    this.standbyMisses = 0;
  }

  private dropStandby(link: Link): void {
    this.standbyFailed();
    this.letGoStandby(link);
  }

  private letGoStandby(link: Link): void {
    this.detach(link);
    this.standby = null;
    this.standbyEvent('unsuitable');
    this.scheduleStandby();
  }

  /**
   * Acts on what the replicas report about committing, once the reports
   * have held for COMMIT_SETTLE_MS. A primary that cannot commit is left
   * only for a standby that reports it can. A standby that cannot commit
   * is let go, to look for a better one, while the primary can, or while
   * a report names a third replica that can, so the client finds that
   * one. With no replica able to commit the client stays on its primary
   * and keeps its standby, so nothing fails over or reconnects until one
   * reports it can.
   */
  private checkCommitable(): void {
    const p = this.primary;
    if (!p || !TRANSMITTING.has(this.link)) return;
    const due = () => {
      if (this.commitable(p) === false && this.promotable()) return 'failover';
      const sb = this.standby;
      if (sb && this.standbyState !== 'Connecting' && this.commitable(sb) === false && (this.commitable(p) !== false || this.thirdCommitable())) return 'drop';
      return null;
    };
    if (due() === null) {
      this.cancelCommitSettle?.();
      this.cancelCommitSettle = null;
      return;
    }
    this.cancelCommitSettle ??= this.deadline(() => {
      this.cancelCommitSettle = null;
      if (p !== this.primary || !TRANSMITTING.has(this.link)) return;
      const act = due();
      if (act === 'failover') this.failover('notCommitable');
      else if (act === 'drop') this.dropStandby(this.standby!);
    }, COMMIT_SETTLE_MS + this.slack());
  }

  /** A report of the primary or the standby names a replica other than theirs that can commit. */
  private thirdCommitable(): boolean {
    const mine = new Set([this.primary?.replica?.id, this.standby?.replica?.id]);
    return [this.primary, this.standby].some((l) => l?.replicas?.some((r) => r.commitable && !mine.has(r.id)));
  }

  private linkEvent(e: LinkEvent): void {
    this.link = step(LINK, this.link, e);
  }

  private standbyEvent(e: StandbyEvent): void {
    this.standbyState = step(STANDBY, this.standbyState, e);
  }

  // ---- server messages --------------------------------------------------------

  private message(link: Link, data: unknown): void {
    link.lastRx = this.clock.now();
    if (link.suspect) {
      link.suspect = false;
      if (link === this.primary) this.linkEvent('beat');
    }
    if (typeof data !== 'string') {
      if (link === this.primary) this.meterFrame(data as ArrayBuffer | ArrayBufferView);
      return;
    }
    let msg: ServerMessage;
    try {
      msg = JSON.parse(data) as ServerMessage;
    } catch {
      return;
    }
    link.replica = msg.replica;
    if (link === this.primary) this.lastPrimaryReplica = msg.replica.id;
    switch (msg.type) {
      case 'ping':
        return this.send(link, { type: 'pong', t: msg.t });
      case 'pong': {
        // A sample only for a ping this client sent on this link, whose t
        // is this clock's time it was sent, floored; earlier pings still
        // unanswered will not be.
        const i = link.pings.indexOf(msg.t);
        if (i < 0) return;
        link.pings.splice(0, i + 1);
        return this.rtt.sample(this.clock.now() - msg.t);
      }
      case 'replicas':
        link.replicas = msg.replicas;
        link.build = msg.build;
        if (link === this.standby && this.standbyState !== 'Connecting' && !this.standbySuitable(link)) {
          return this.missStandby(link);
        }
        return this.checkCommitable();
      case 'snapshot':
        if (link === this.primary) return this.primarySnapshot(msg);
        // A standby on another history would hand this state foreign deltas.
        return msg.epoch === this.epoch ? this.standbySynced(link) : this.dropStandby(link);
      case 'resume':
        return link === this.primary ? this.primaryResume(msg) : this.standbyResume(link, msg);
      case 'delta':
        return this.delta(msg, link);
      case 'ack':
        return this.answer(msg.commandId, msg.result === 'stale' ? 'stale' : 'ack');
      case 'error':
        return this.error(msg);
      case 'ahead':
        if (link === this.primary) this.primaryAhead(msg);
        return;
      case 'counters':
        if (link === this.primary) this.counters = { replica: msg.replica.id, values: msg.values };
        return;
      case 'selections':
        // Every connection keeps its copy; a promoted standby's is shown at once.
        link.selections = msg.selections;
        if (link === this.primary) this.selectionsDirty = true;
        return;
      case 'presence':
        // Each replica lists its own connections; the primary's are shown.
        link.presence = msg.sessions;
        if (link === this.primary) this.presenceDirty = true;
        return;
      case 'controlRequest':
        if (link === this.primary) this.notice({ kind: 'request', path: msg.path, from: msg.from });
        return;
    }
  }

  private primarySnapshot(msg: SnapshotMessage): void {
    // Taking the cluster state is the one case where state moves backwards
    // or onto another history.
    if (!this.adopting && this.state && msg.epoch !== this.epoch) return this.otherHistory(msg);
    if (!this.adopting && this.older(msg.revision)) return;
    this.adopting = false;
    this.ahead = null;
    this.epoch = msg.epoch;
    this.you = msg.you;
    this.shape = msg.shape;
    this.state = msg.state;
    this.owners = msg.owners;
    this.revision = msg.revision;
    this.layout = deriveMeterLayout(msg.shape);
    this.layoutResyncSent = false;
    this.snapshotPending = true;
    this.dirty.clear();
    this.ownersDirty.clear();
    this.meters = null;
    // A snapshot can skip the deltas that answered commands sent before the
    // hello or resync it answers; they go out again with their ids and the
    // server answers each one. A command sent after that request is read
    // after it and answered after the snapshot, so it is not sent again.
    // The request it answers is the last one sent: the engine sends a
    // resync only once the previous request is answered or its
    // connection is left. A snapshot that arrives with no request
    // unanswered answers none (the server's queue for this client filled
    // with deltas), so it can skip the answer to every sent command.
    const answering = this.link === 'Syncing' || this.link === 'Resyncing';
    this.outbox.failover(answering ? this.syncRequests : Infinity);
    this.primarySynced('snapshot');
  }

  private primaryResume(msg: ResumeMessage): void {
    if (this.state && msg.epoch !== this.epoch) return this.resync(true);
    if (this.older(msg.revision)) return;
    this.you = msg.you;
    if (!this.state) return this.resync(true);
    for (const d of msg.deltas) {
      if (d.revision <= this.revision) continue;
      if (d.revision !== this.revision + 1) return this.resync(true);
      this.apply(d);
    }
    if (this.revision !== msg.revision) return this.resync(true);
    this.primarySynced('resume');
  }

  private standbyResume(link: Link, msg: ResumeMessage): void {
    if (msg.epoch !== this.epoch) return this.dropStandby(link);
    for (const d of msg.deltas) this.delta(d, link);
    this.standbySynced(link);
  }

  private standbySynced(link: Link): void {
    if (!this.standbySuitable(link)) return this.missStandby(link);
    this.standbyEvent('synced');
    this.standbyAttempts = 0;
    this.standbyFailures = 0;
    this.standbyMisses = 0;
    this.checkCommitable();
  }

  /**
   * After a snapshot or resume on the primary: release every control this
   * session owns but is not touching, touch again what it is touching but
   * no longer owns, send the queued commands and open a standby.
   */
  private primarySynced(kind: 'snapshot' | 'resume'): void {
    this.cancelSyncTimeout?.();
    this.cancelSyncTimeout = null;
    this.lastSync = kind;
    this.primaryAttempts = 0;
    this.offlineRounds = 0;
    this.syncTimeouts = 0;
    this.linkEvent('synced');
    const replica = this.primary?.replica?.id ?? null;
    if (this.failoverInfo && this.failoverInfo.to === null) this.failoverInfo = { ...this.failoverInfo, to: replica };
    for (const path of new Set([...Object.keys(this.owners), ...this.touches.keys()])) this.reconcile(path);
    this.sendSelection();
    this.transmit();
    if (this.standby && this.standby.replica && !this.standbySuitable(this.standby)) this.missStandby(this.standby);
    this.startStandby();
    this.checkCommitable();
  }

  /**
   * State never moves backwards: an answer older than the applied revision
   * is dropped and asked for again. Replicas answer only once they have
   * applied the revision a client sends (read index), so this is a guard.
   */
  private older(revision: number): boolean {
    if (!this.state || revision >= this.revision) return false;
    this.cancelResyncRetry?.();
    this.cancelResyncRetry = this.clock.schedule(() => {
      this.cancelResyncRetry = null;
      const p = this.primary;
      if (p && (this.link === 'Resyncing' || this.link === 'Syncing')) this.send(p, { type: 'resync', ...this.position() });
    }, RESYNC_RETRY_MS);
    return true;
  }

  /**
   * The replica will not answer with state: this client holds a revision
   * the cluster never committed. Nothing is retried; the operator decides.
   */
  private primaryAhead(msg: AheadMessage): void {
    this.cancelResyncRetry?.();
    this.cancelResyncRetry = null;
    this.ahead = { lastRevision: msg.lastRevision, cluster: msg.revision, otherHistory: false };
    this.linkEvent('ahead');
    this.closeStandby();
  }

  /**
   * The cluster answered with a snapshot of another epoch: it was restored
   * from a backup or started empty, and this state is not in its history,
   * whatever the revision numbers say. Same as ahead: nothing is applied
   * and nothing is retried until the operator takes the cluster state.
   */
  private otherHistory(msg: SnapshotMessage): void {
    this.cancelResyncRetry?.();
    this.cancelResyncRetry = null;
    this.ahead = { lastRevision: this.revision, cluster: msg.revision, otherHistory: true };
    this.linkEvent('ahead');
    this.closeStandby();
  }

  /** Takes the cluster state: a resync without lastRevision, whose snapshot replaces this state. */
  private adopt(): void {
    const p = this.primary;
    if (this.link !== 'Ahead' || !p) return;
    this.adopting = true;
    this.send(p, { type: 'resync' });
    this.linkEvent('adopt');
    this.awaitSync();
    this.closeStandby();
  }

  /**
   * Closes the standby in whatever state it is in; the snapshot that ends
   * Ahead opens a new one.
   */
  private closeStandby(): void {
    if (!this.standby) return;
    this.detach(this.standby);
    this.standby = null;
    this.standbyEvent('lost');
  }

  private resync(full: boolean): void {
    const p = this.primary;
    if (!p) return;
    this.send(p, { type: 'resync', ...this.position(), ...(full && this.state ? { full: true as const } : {}) });
    this.linkEvent('resync');
    this.awaitSync();
  }

  private delta(d: DeltaMessage, link: Link): void {
    // While Ahead the connections may carry another history's deltas.
    if (!this.state || this.link === 'Ahead' || d.revision <= this.revision) return;
    if (d.revision !== this.revision + 1) {
      if (link === this.primary && (this.link === 'Live' || this.link === 'Suspect')) this.resync(false);
      return;
    }
    this.apply(d);
  }

  private apply(d: DeltaMessage): void {
    const state = this.state as unknown as Record<string, unknown>;
    for (const c of d.changes ?? []) {
      setAt(state, c.path, c.value);
      this.dirty.add(c.path);
    }
    for (const o of d.owners ?? []) {
      if (o.owner === null) delete this.owners[o.path];
      else this.owners[o.path] = o.owner;
      this.ownersDirty.add(o.path);
    }
    this.revision = d.revision;
    for (const o of d.origins ?? []) if (this.mine(o.owner)) this.answer(o.commandId, 'ack');
  }

  private error(msg: ErrorMessage): void {
    if (msg.code === 503 && msg.commandId) {
      // Not applied: nothing changed, and the command goes out again with its id.
      this.outbox.notApplied(msg.commandId, this.clock.now() + NOT_APPLIED_RETRY_MS);
      return;
    }
    if (msg.commandId) this.answer(msg.commandId, 'reject');
    if (msg.code === 409 && msg.path) this.notice({ kind: 'conflict', path: msg.path, owner: msg.owner ?? null });
    else this.notice({ kind: 'error', text: msg.message });
  }

  private notice(n: WorkerNotice): void {
    this.notices.push(n);
    if (this.notices.length > NOTICE_LIMIT) this.notices.shift();
  }

  private meterFrame(data: ArrayBuffer | ArrayBufferView): void {
    this.credits = Math.max(0, this.credits - 1);
    const layout = this.layout;
    if (!layout) return;
    try {
      const f = decodeMeterFrame(layout, data);
      this.meters = { replica: f.replica, sequence: f.sequence, values: flattenMeterFrame(layout, f) };
    } catch (e) {
      if (!(e instanceof MeterFrameError) || e.reason !== 'fingerprint' || this.layoutResyncSent) return;
      // Not while a hello or resync is unanswered: its answer may bring
      // the new layout, and one request at a time tells a snapshot which
      // commands it answers for (primarySnapshot).
      if (this.link !== 'Live' && this.link !== 'Suspect') return;
      this.layoutResyncSent = true;
      this.resync(true);
    }
  }

  // ---- page ticks --------------------------------------------------------------

  private grantCredits(): void {
    const p = this.primary;
    if (!p?.opened || !TRANSMITTING.has(this.link) || this.credits > CREDIT_REFILL_AT) return;
    const n = METER_CREDIT_CAP - this.credits;
    this.send(p, { type: 'credit', n });
    this.credits = METER_CREDIT_CAP;
  }

  status(): SyncStatus {
    return {
      link: this.link,
      standby: this.standbyState,
      session: this.deps.session,
      handle: this.handle,
      you: this.you,
      primaryReplica: this.primary?.replica?.id ?? null,
      primaryEndpoint: this.primary ? this.deps.endpoints[this.primary.endpoint]! : null,
      standbyReplica: this.standby?.replica?.id ?? null,
      pendingKeys: this.outbox.discreteSetPaths(),
      replicas: this.primary?.replicas ?? this.standby?.replicas ?? [],
      build: this.primary?.build ?? null,
      revision: this.revision,
      outbox: this.outbox.size + this.limiter.size,
      failover: this.failoverInfo,
      touching: [...this.touches].filter(([, s]) => s === 'Touching').map(([p]) => p),
      lastSync: this.lastSync,
      meterCredits: this.credits,
      ahead: this.ahead,
    };
  }

  private postPatch(): void {
    const patch: Patch = { type: 'patch', input: this.inputs, changes: [], owners: [] };
    const transfer: Transferable[] = [];
    // A command is answered by a patch even when it changed nothing that
    // the page shows, so the page knows that its outbox covers it.
    let any = this.commanded;
    this.commanded = false;
    if (this.snapshotPending && this.state && this.shape) {
      this.snapshotPending = false;
      patch.snapshot = { shape: this.shape, state: this.state, owners: this.owners };
      for (const [path, value] of this.localValue) patch.changes.push([path, value]);
      this.dirty.clear();
      this.ownersDirty.clear();
      any = true;
    }
    for (const path of this.dirty) {
      if (!this.pending(path)) patch.changes.push([path, getAt(this.state, path)]);
    }
    this.dirty.clear();
    for (const path of this.ownersDirty) patch.owners.push([path, this.owners[path] ?? null]);
    this.ownersDirty.clear();
    any ||= patch.changes.length > 0 || patch.owners.length > 0;
    const status = this.status();
    const text = JSON.stringify(status);
    if (text !== this.lastStatus) {
      this.lastStatus = text;
      patch.status = status;
      any = true;
    }
    if (this.meters) {
      patch.meters = this.meters;
      transfer.push(this.meters.values.buffer);
      this.meters = null;
      any = true;
    }
    if (this.counters) {
      patch.counters = this.counters;
      this.counters = null;
      any = true;
    }
    if (this.selectionsDirty) {
      this.selectionsDirty = false;
      patch.selections = this.primary?.selections ?? [];
      any = true;
    }
    if (this.presenceDirty) {
      this.presenceDirty = false;
      patch.presence = this.primary?.presence ?? [];
      any = true;
    }
    if (this.notices.length > 0) {
      patch.notices = this.notices;
      this.notices = [];
      any = true;
    }
    if (any) this.deps.post(patch, transfer);
  }
}
