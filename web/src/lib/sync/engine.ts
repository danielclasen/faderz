// Sync engine of the web client, run by the sync worker. Keeps a primary
// WebSocket and a standby one to another replica, heartbeats both, detects
// a dead or non-committing primary and promotes the standby, holds the
// replicated state, the command outbox and the meter credits, and builds
// one patch per page tick. Pure: sockets, clock and posting are injected,
// so it runs in a worker and under a fake clock in tests.

import { decodeMeterFrame, deriveMeterLayout, MeterFrameError, type MeterLayout } from '../contract/meterFrame';
import { DEAD_MS, HEARTBEAT_MS, METER_CREDIT_CAP, SUSPECT_MS } from '../contract/protocol';
import type {
  ClientMessage,
  CommandMessage,
  DeltaMessage,
  Epoch,
  ErrorMessage,
  Owner,
  Owners,
  ReplicaRef,
  ReplicaStatus,
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
}

/** A socket that has not opened by then is abandoned. */
export const CONNECT_TIMEOUT_MS = 2000;
/** Delay before the next primary attempt, by consecutive failures. */
export const PRIMARY_RETRY_MS: readonly number[] = [0, 250, 500];
/** Consecutive failed primary attempts that make the link Offline. */
export const PRIMARY_ATTEMPTS = 3;
export const OFFLINE_RETRY_MS = 2000;
/** Standby attempts in a row before the standby backs off. */
export const STANDBY_TRIES = 4;
export const STANDBY_RETRY_MS = 100;
export const STANDBY_BACKOFF_MS = 2000;
/** Delay before asking again after an answer older than the client's revision. */
export const RESYNC_RETRY_MS = 250;
/** Credits are topped up to the cap once this many or fewer are outstanding. */
export const CREDIT_REFILL_AT = METER_CREDIT_CAP / 2;
const NOTICE_LIMIT = 20;

type Role = 'primary' | 'standby';

class Link {
  replica: ReplicaRef | null = null;
  replicas: ReplicaStatus[] | null = null;
  opened = false;
  suspect = false;
  lastRx: number;
  cancelWatch: (() => void) | null = null;

  constructor(
    public role: Role,
    readonly endpoint: number,
    readonly socket: SocketLike,
    now: number,
  ) {
    this.lastRx = now;
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

  private readonly clock: Clock;
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
  private lastPrimaryReplica: string | null = null;
  private failoverInfo: FailoverInfo | null = null;
  private lastSync: SyncStatus['lastSync'] = null;
  private inputs = 0;
  private cancelRetry: (() => void) | null = null;
  private cancelStandbyRetry: (() => void) | null = null;
  private cancelResyncRetry: (() => void) | null = null;

  constructor(private readonly deps: EngineDeps) {
    if (deps.endpoints.length === 0) throw new Error('no endpoints');
    this.clock = deps.clock;
    this.handle = ownerHandle(deps.session);
    this.limiter = new RateLimiter(this.clock, (path, value) => this.issueSet(path, value));
  }

  start(): void {
    this.openPrimary();
    const beat = () => {
      const t = Math.max(0, Math.floor(this.clock.now()));
      for (const l of [this.primary, this.standby]) if (l?.opened) this.send(l, { type: 'ping', t });
      this.clock.schedule(beat, HEARTBEAT_MS);
    };
    this.clock.schedule(beat, HEARTBEAT_MS);
  }

  input(msg: ToWorker): void {
    this.inputs++;
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

  private issueSet(path: string, value: unknown): void {
    const seq = (this.seq.get(path) ?? this.deps.seqBase) + 1;
    this.seq.set(path, seq);
    this.issue({ type: 'set', id: uuid(), seq, path, value });
  }

  private issue(msg: CommandMessage): void {
    const r = this.outbox.enqueue(msg, msg.type === 'set' && isContinuousPath(msg.path));
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
    for (const m of this.outbox.transmit()) this.send(p, m);
  }

  private answer(id: string, event: 'ack' | 'reject' | 'stale'): void {
    const e = this.outbox.answer(id, event);
    if (!e) return;
    const m = e.msg;
    if (m.type === 'touch' && event === 'reject') this.touchStep(m.path, 'refused');
    if (m.type === 'release' && !this.outbox.has('release', m.path)) this.touchStep(m.path, 'released');
    this.settled(m);
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

  private touchEvent(path: string, event: 'down' | 'up'): void {
    const prev = this.touches.get(path) ?? 'Idle';
    if (this.touchStep(path, event) === prev) return;
    this.issue({ type: event === 'down' ? 'touch' : 'release', id: uuid(), path });
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
    link.cancelWatch = this.clock.schedule(() => this.closed(link, 'lost'), CONNECT_TIMEOUT_MS);
    socket.onopen = () => this.opened(link);
    socket.onmessage = (ev) => this.message(link, ev.data);
    socket.onclose = () => this.closed(link, 'lost');
    socket.onerror = () => {
      // onclose follows and handles the failure.
    };
    return link;
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
    if (link.opened) link.socket.send(JSON.stringify(msg));
  }

  private opened(link: Link): void {
    link.opened = true;
    link.lastRx = this.clock.now();
    this.watch(link);
    if (link.role === 'primary') {
      this.credits = 0;
      this.linkEvent('open');
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

  /** Re-arms the traffic watch of a link: suspect after SUSPECT_MS, dead after DEAD_MS. */
  private watch(link: Link): void {
    link.cancelWatch?.();
    const due = link.lastRx + (link.suspect ? DEAD_MS : SUSPECT_MS) - this.clock.now();
    link.cancelWatch = this.clock.schedule(() => {
      const silence = this.clock.now() - link.lastRx;
      if (silence >= DEAD_MS) return this.closed(link, 'dead');
      if (silence >= SUSPECT_MS && !link.suspect) {
        link.suspect = true;
        if (link === this.primary) this.linkEvent('beatLate');
      }
      this.watch(link);
    }, due);
  }

  private closed(link: Link, why: 'lost' | 'dead'): void {
    if (link !== this.primary && link !== this.standby) return;
    this.detach(link);
    if (link === this.standby) {
      this.standby = null;
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
      this.cancelRetry = this.clock.schedule(() => {
        this.primaryAttempts = 0;
        this.linkEvent('retry');
        this.openPrimary();
      }, OFFLINE_RETRY_MS);
      return;
    }
    this.cancelRetry = this.clock.schedule(() => this.openPrimary(), PRIMARY_RETRY_MS[this.primaryAttempts] ?? 0);
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
    if (sb && this.standbyState === 'Ready') {
      this.standbyEvent('promote');
      this.standby = null;
      sb.role = 'primary';
      this.primary = sb;
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
    const delay = this.standbyAttempts < STANDBY_TRIES ? STANDBY_RETRY_MS : STANDBY_BACKOFF_MS;
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

  private standbySuitable(link: Link): boolean {
    const id = link.replica?.id;
    if (!id || id === this.primary?.replica?.id) return false;
    return link.replicas?.find((r) => r.id === id)?.commitable !== false;
  }

  private dropStandby(link: Link): void {
    this.detach(link);
    this.standby = null;
    this.standbyEvent('unsuitable');
    this.scheduleStandby();
  }

  private checkCommitable(): void {
    const p = this.primary;
    if (!p || !TRANSMITTING.has(this.link) || this.standbyState !== 'Ready') return;
    const own = p.replicas?.find((r) => r.id === p.replica?.id);
    if (own && !own.commitable) this.failover('notCommitable');
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
      case 'pong':
        return;
      case 'replicas':
        link.replicas = msg.replicas;
        if (link === this.standby && this.standbyState !== 'Connecting' && !this.standbySuitable(link)) {
          return this.dropStandby(link);
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
    // A snapshot can skip the deltas that answered sent commands; they go
    // out again with their ids and the server answers each one.
    this.outbox.failover();
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
    if (!this.standbySuitable(link)) return this.dropStandby(link);
    this.standbyEvent('synced');
    this.standbyAttempts = 0;
    this.checkCommitable();
  }

  /**
   * After a snapshot or resume on the primary: release every control this
   * session owns but is not touching, touch again what it is touching but
   * no longer owns, send the queued commands and open a standby.
   */
  private primarySynced(kind: 'snapshot' | 'resume'): void {
    this.lastSync = kind;
    this.primaryAttempts = 0;
    this.linkEvent('synced');
    const replica = this.primary?.replica?.id ?? null;
    if (this.failoverInfo && this.failoverInfo.to === null) this.failoverInfo = { ...this.failoverInfo, to: replica };
    for (const [path, owner] of Object.entries(this.owners)) {
      if (this.mine(owner) && this.touches.get(path) !== 'Touching' && !this.outbox.has('release', path)) {
        this.issue({ type: 'release', id: uuid(), path });
      }
    }
    for (const [path, s] of this.touches) {
      if (s === 'Touching' && !this.mine(this.owners[path]) && !this.outbox.has('touch', path)) {
        this.issue({ type: 'touch', id: uuid(), path });
      }
    }
    this.transmit();
    if (this.standby && this.standby.replica && !this.standbySuitable(this.standby)) this.dropStandby(this.standby);
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
      replicas: this.primary?.replicas ?? this.standby?.replicas ?? [],
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
    let any = false;
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
    if (this.notices.length > 0) {
      patch.notices = this.notices;
      this.notices = [];
      any = true;
    }
    if (any) this.deps.post(patch, transfer);
  }
}
