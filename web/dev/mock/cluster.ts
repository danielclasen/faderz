// Dev-only mock of a faderz replica set: one replicated state with a
// global revision, several replicas serving primary and standby
// connections, heartbeats, session leases for touch ownership, commands
// batched per block into log entries, idempotent command ids, scenes, AFV
// tally and console clients. Every message it sends or receives is
// checked against the committed contract.

import type {
  AckMessage,
  AfvBinding,
  Change,
  ClientMessage,
  CommandMessage,
  CommandResult,
  Conflict,
  DeltaMessage,
  ErrorMessage,
  LinkMode,
  Origin,
  Owner,
  OwnershipChange,
  RecallGroup,
  ReplicaRef,
  ReplicaRole,
  ReplicaStatus,
  Scene,
  ServerMessage,
  Shape,
  State,
  TallyState,
} from '../../src/lib/contract/types.ts';
import { LOUDNESS_PRESETS } from '../../src/lib/contract/types.ts';
import { decodeMeterFrame, deriveMeterLayout, encodeMeterFrame, type MeterLayout } from '../../src/lib/contract/meterFrame.ts';
import {
  DEAD_MS,
  HEARTBEAT_MS,
  LEASE_MS,
  METER_CREDIT_CAP,
  METER_PERIOD_MS,
} from '../../src/lib/contract/protocol.ts';
import { getAt, parsePointer, pointer, setAt } from '../../src/lib/state/pointer.ts';
import { Contract, ContractViolation } from './contract.ts';
import { CONSOLE_SESSIONS, exampleShape, initialState } from './shape.ts';
import { ReplicaMeters, SourceModel } from './signals.ts';

/** Commands arriving within one block share a log entry. */
export const BLOCK_MS = 5;
const LOG_LIMIT = 1000;
const SAMPLES_PER_MS = 48;
/** Scheduling margin between accepting a change and the sample it applies at. */
const APPLY_MARGIN_SAMPLES = 480;

type LogEntry = Omit<DeltaMessage, 'type' | 'replica'>;

/** Traffic counters of one connection, reset by resetStats(). */
export interface ConnStats {
  meterFramesSent: number;
  /** Meter periods in which this primary connection held no credit. */
  metersSkippedNoCredit: number;
  messagesIn: number;
  messagesOut: number;
}

export interface Conn {
  id: string;
  /** ?session= of the WebSocket upgrade. */
  session: string;
  replica: Replica;
  owner: Owner | null;
  mode: LinkMode;
  synced: boolean;
  /** Outstanding meter credits, 0 to METER_CREDIT_CAP. */
  credit: number;
  /** Date.now() of the last message received on the connection. */
  lastSeen: number;
  /** lastRevision of the promote: later commands wait until the replica applied it. */
  floor: number;
  openedAt: number;
  stats: ConnStats;
  send(data: string | Uint8Array): void;
  close(): void;
}

export interface Replica {
  id: string;
  role: ReplicaRole;
  healthy: boolean;
  /** none: serving; drop: hung, ignores input; apply: hung after replicating input. */
  frozen: 'none' | 'drop' | 'apply';
  /** Cut off from the quorum: serves reads and heartbeats but cannot commit. */
  partitioned: boolean;
  /** Log entries reach this replica lagMs late; 0 applies them at once. */
  lagMs: number;
  /** Revision this replica has applied. */
  applied: number;
  /** State and owners at `applied` while lagging; null when they are the cluster's. */
  view: { state: State; owners: Record<string, Owner> } | null;
  /** Answers and commands held until `applied` reaches their floor (read index). */
  waiting: Array<{ floor: number; run: () => void }>;
  conns: Set<Conn>;
  meters: ReplicaMeters;
  /** Meter period counter, uint32, sent as the frame sequence. */
  meterSequence: number;
  counters: Record<string, number>;
}

/** One command applied to the state, as GET /__mock/audit lists it. */
export interface AppliedCommand {
  owner: Owner;
  commandId: string;
  type: CommandMessage['type'];
  /** seq of a set; null for touch and release. */
  seq: number | null;
  path: string;
  /** Value of a set; null for touch and release. */
  value: unknown;
  revision: number;
  replica: string;
}

export interface AuditEntry {
  commandId: string;
  owner: Owner | null;
  type: CommandMessage['type'] | 'rest';
  path: string | null;
  result: 'applied' | 'duplicate' | 'stale' | 'noop' | 'conflict';
  revision: number;
  replica: string;
}

export class CommandError extends Error {
  readonly code: 400 | 404 | 409;
  readonly conflicts: Conflict[];

  constructor(code: 400 | 404 | 409, message: string, conflicts: Conflict[] = []) {
    super(message);
    this.code = code;
    this.conflicts = conflicts;
  }
}

export const sessionKey = (o: Pick<Owner, 'principal' | 'session'>) => `${o.principal}\n${o.session}`;

function sampleIndexNow(): number {
  return Math.floor(Date.now() * SAMPLES_PER_MS) + APPLY_MARGIN_SAMPLES;
}

function equal(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * Leaves of a state subtree. Arrays of strings (id sets, recall groups)
 * count as one leaf; per-leg arrays split into one leaf per leg.
 */
function leaves(value: unknown, base: string, out: Change[] = []): Change[] {
  if (Array.isArray(value) && value.every((v) => typeof v === 'string')) {
    out.push({ path: base, value });
  } else if (value !== null && typeof value === 'object') {
    for (const [k, v] of Object.entries(value)) leaves(v, `${base}${pointer(k)}`, out);
  } else {
    out.push({ path: base, value });
  }
  return out;
}

/** Recall-safe group a state leaf belongs to; null for leaves scenes never touch. */
export function recallGroup(path: string): RecallGroup | null {
  const [group, , key] = parsePointer(path);
  if (group === 'afv') return key === 'open' ? null : 'afv';
  if (group === 'mixMinus') return key === 'exclude' ? 'routing' : key === 'interrupt' ? null : (key as RecallGroup);
  if (group === 'vcas') return key === 'offset' ? 'fader' : key === 'members' ? 'routing' : key === 'solo' ? null : 'mute';
  switch (key) {
    case 'input': case 'trim': case 'legTrim': case 'polarity': case 'hpf': case 'delay': return 'input';
    case 'eq': return 'eq';
    case 'gate': case 'comp': case 'dynamicsBeforeEq': case 'limiter': return 'dynamics';
    case 'pan': case 'image': return 'pan';
    case 'sends': return 'sends';
    case 'routes': case 'directOut': return 'routing';
    case 'fader': return 'fader';
    case 'mute': return 'mute';
    default: return null;
  }
}

interface StoredScene {
  scene: Scene;
  data: Pick<State, 'channels' | 'buses' | 'auxes' | 'mixMinus' | 'vcas'> & { afv: Record<string, Omit<AfvBinding, 'open'>> };
}

export interface ClusterOptions {
  replicas?: number;
  /** Scripted tally cuts and console activity; off for deterministic tests. */
  automation?: boolean;
  /** Input channels of the generated shape. */
  channels?: number;
}

interface Batch {
  revision: number;
  atIndex?: number;
  changes: Change[];
  owners: OwnershipChange[];
  origins: Origin[];
}

const CONSOLE_OWNER: Owner = { principal: 'bridge', session: CONSOLE_SESSIONS['mcu-1']!, kind: 'console', connection: 'bridge-mcu-1' };

const emptyStats = (): ConnStats => ({ meterFramesSent: 0, metersSkippedNoCredit: 0, messagesIn: 0, messagesOut: 0 });

export class Cluster {
  readonly contract = new Contract();
  readonly shape: Shape;
  state: State;
  owners: Record<string, Owner> = {};
  revision = 0;
  readonly replicas: Replica[];
  readonly audit: AuditEntry[] = [];
  /** Every command applied to the state, in apply order. */
  readonly applies: AppliedCommand[] = [];
  /** Counters over every connection since start or resetStats(). */
  readonly totals: ConnStats = emptyStats();
  readonly meterLayout: MeterLayout;
  private log: LogEntry[] = [];
  private logStart = 1;
  private batch: Batch | null = null;
  private batchTimer: ReturnType<typeof setTimeout> | null = null;
  /** Blocks end only on flush(), a hello or a resync: dev control for batching tests. */
  private holdBlocks = false;
  private lagTimers = new Set<ReturnType<typeof setTimeout>>();
  private applied = new Map<string, number>();
  private lastSeq = new Map<string, number>();
  private leases = new Map<string, number>();
  private scenes = new Map<string, StoredScene>();
  private sceneCounter = 0;
  private connCounter = 0;
  private timers: Array<ReturnType<typeof setInterval>> = [];
  private readonly sources: SourceModel;
  private readonly automation: boolean;

  constructor(options: ClusterOptions = {}) {
    this.shape = exampleShape(options.channels);
    this.meterLayout = deriveMeterLayout(this.shape);
    this.state = initialState(this.shape);
    this.sources = new SourceModel(this.shape);
    this.automation = options.automation ?? true;
    const n = options.replicas ?? 3;
    this.replicas = Array.from({ length: n }, (_, i) => ({
      id: `r${i + 1}`,
      role: i === 0 ? 'leader' : 'follower',
      healthy: true,
      frozen: 'none',
      partitioned: false,
      lagMs: 0,
      applied: 0,
      view: null,
      waiting: [],
      conns: new Set<Conn>(),
      meters: new ReplicaMeters(this.shape),
      meterSequence: 0,
      counters: {
        deadline_miss_total: 0,
        overrun_skipped_samples_total: 0,
        input_late_blocks_total: 0,
        input_wait_timeout_total: 0,
        command_late_total: 0,
        afv_late_total: 0,
      },
    }));
    this.storeScene('Opening', null);
    this.storeScene('Interview', null);
    this.flush();
  }

  // ---- lifecycle ----------------------------------------------------------

  start(): void {
    this.timers.push(setInterval(() => this.meterTick(), METER_PERIOD_MS));
    this.timers.push(setInterval(() => this.counterTick(), 1000));
    this.timers.push(setInterval(() => this.heartbeatTick(), HEARTBEAT_MS));
    if (this.automation) {
      let cam = 1;
      this.timers.push(
        setInterval(() => {
          cam = (cam % 4) + 1;
          const next = (cam % 4) + 1;
          const sources: Record<string, TallyState> = { CAM1: 'off', CAM2: 'off', CAM3: 'off', CAM4: 'off' };
          sources[`CAM${cam}`] = 'program';
          sources[`CAM${next}`] = 'preview';
          this.applyTally(sources, null);
        }, 8000),
      );
      this.timers.push(setInterval(() => this.consoleBank(), 30000));
      this.timers.push(setInterval(() => this.consoleMotion(), 25000));
    }
  }

  stop(): void {
    for (const t of this.timers) clearInterval(t);
    this.timers = [];
    if (this.batchTimer) clearTimeout(this.batchTimer);
    for (const t of this.lagTimers) clearTimeout(t);
    for (const r of this.replicas) for (const c of r.conns) c.close();
  }

  ref(r: Replica): ReplicaRef {
    return { id: r.id, role: r.role };
  }

  private ready(r: Replica): boolean {
    return r.healthy && r.frozen === 'none';
  }

  replicaStatus(): ReplicaStatus[] {
    const quorum = this.replicas.filter((r) => this.ready(r) && !r.partitioned).length > this.replicas.length / 2;
    return this.replicas.map((r) => ({
      id: r.id,
      role: r.role,
      healthy: this.ready(r),
      commitable: this.ready(r) && !r.partitioned && quorum,
    }));
  }

  /** Replica a service endpoint routes a new connection to: any ready one. */
  route(): Replica | null {
    const ready = this.replicas.filter((r) => this.ready(r) && !r.partitioned);
    if (ready.length === 0) return null;
    return ready[Math.floor(Math.random() * ready.length)]!;
  }

  replica(id: string): Replica | undefined {
    return this.replicas.find((r) => r.id === id);
  }

  kill(id: string): boolean {
    const r = this.replica(id);
    if (!r) return false;
    r.healthy = false;
    r.frozen = 'none';
    for (const c of [...r.conns]) c.close();
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  freeze(id: string, mode: 'drop' | 'apply'): boolean {
    const r = this.replica(id);
    if (!r || !r.healthy) return false;
    r.frozen = mode;
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  /** A hung replica resumes on its open connections; what it skipped stays skipped. */
  unfreeze(id: string): boolean {
    const r = this.replica(id);
    if (!r || !r.healthy) return false;
    r.frozen = 'none';
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  partition(id: string): boolean {
    const r = this.replica(id);
    if (!r || !r.healthy) return false;
    r.partitioned = true;
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  heal(id: string): boolean {
    const r = this.replica(id);
    if (!r) return false;
    r.partitioned = false;
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  /** Back to serving: restarts a killed replica, closes the connections of a hung one, heals a partition. */
  restore(id: string): boolean {
    const r = this.replica(id);
    if (!r) return false;
    for (const c of [...r.conns]) if (r.frozen !== 'none') c.close();
    if (!r.healthy) r.meterSequence = 0;
    r.healthy = true;
    r.frozen = 'none';
    r.partitioned = false;
    this.lag(id, 0);
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  private elect(): void {
    const ok = (r: Replica) => this.ready(r) && !r.partitioned;
    const leader = this.replicas.find((r) => r.role === 'leader');
    if (leader && ok(leader)) return;
    const next = this.replicas.find(ok);
    if (!next) return;
    for (const r of this.replicas) r.role = r === next ? 'leader' : 'follower';
  }

  // ---- connections --------------------------------------------------------

  connect(replica: Replica, session: string, send: (data: string | Uint8Array) => void, close: () => void): Conn {
    const now = Date.now();
    const conn: Conn = {
      id: `c${++this.connCounter}`,
      session,
      replica,
      owner: null,
      mode: 'primary',
      synced: false,
      credit: 0,
      lastSeen: now,
      floor: 0,
      openedAt: now,
      stats: emptyStats(),
      send,
      close,
    };
    replica.conns.add(conn);
    return conn;
  }

  /** The session's lease runs on; nothing is released when one connection goes. */
  disconnect(conn: Conn): void {
    conn.replica.conns.delete(conn);
  }

  emit(conn: Conn, msg: ServerMessage): void {
    if (!this.ready(conn.replica)) return;
    this.contract.assertServerMessage(msg);
    this.count(conn, 'messagesOut');
    conn.send(JSON.stringify(msg));
  }

  private count(conn: Conn, key: keyof ConnStats): void {
    conn.stats[key]++;
    this.totals[key]++;
  }

  /** Zero the counters of every connection and the totals. */
  resetStats(): void {
    for (const r of this.replicas) for (const c of r.conns) c.stats = emptyStats();
    Object.assign(this.totals, emptyStats());
  }

  /** Clear the command logs of GET /__mock/commands and /__mock/audit. */
  resetAudit(): void {
    this.audit.length = 0;
    this.applies.length = 0;
  }

  connections(): Conn[] {
    return this.replicas.flatMap((r) => [...r.conns]);
  }

  private renew(owner: Owner, replica: Replica): void {
    if (replica.partitioned) return;
    this.leases.set(sessionKey(owner), Date.now());
  }

  /** A message from a client; binary messages are not part of the contract. */
  receive(conn: Conn, text: string, binary = false): void {
    const r = conn.replica;
    this.count(conn, 'messagesIn');
    // Input a hung replica has not read yet still shows the peer alive once it resumes.
    conn.lastSeen = Date.now();
    if (!r.healthy || r.frozen === 'drop') return;
    if (binary) {
      return this.emit(conn, { type: 'error', replica: this.ref(r), code: 400, message: 'binary messages are not accepted' });
    }
    let msg: ClientMessage;
    try {
      msg = JSON.parse(text) as ClientMessage;
    } catch {
      return this.emit(conn, { type: 'error', replica: this.ref(r), code: 400, message: 'not JSON' });
    }
    const problem = this.contract.checkClientMessage(msg);
    if (problem) {
      const commandId = (msg as { id?: unknown }).id;
      return this.emit(conn, {
        type: 'error',
        replica: this.ref(r),
        code: 400,
        message: `invalid message: ${problem}`,
        ...(typeof commandId === 'string' && /^[0-9a-f-]{36}$/i.test(commandId) ? { commandId } : {}),
      });
    }
    if (msg.type === 'hello') {
      if (msg.session !== conn.session) {
        return this.emit(conn, { type: 'error', replica: this.ref(r), code: 400, message: 'hello session differs from ?session=' });
      }
      conn.owner = { principal: 'anonymous', session: msg.session, kind: msg.kind, connection: conn.id };
      conn.mode = msg.mode;
      this.renew(conn.owner, r);
      this.emit(conn, { type: 'replicas', replica: this.ref(r), replicas: this.replicaStatus() });
      return this.sync(conn, msg.lastRevision);
    }
    if (!conn.owner) {
      return this.emit(conn, { type: 'error', replica: this.ref(r), code: 400, message: 'hello first' });
    }
    this.renew(conn.owner, r);
    switch (msg.type) {
      case 'ping':
        return this.emit(conn, { type: 'pong', replica: this.ref(r), t: msg.t });
      case 'pong':
        return;
      case 'promote':
        conn.mode = 'primary';
        conn.floor = msg.lastRevision;
        return;
      case 'credit':
        conn.credit = Math.min(METER_CREDIT_CAP, conn.credit + msg.n);
        return;
      case 'resync':
        return this.sync(conn, msg.lastRevision, msg.full === true);
    }
    // A partitioned replica cannot commit; commands sent to it go unanswered.
    if (r.partitioned) return;
    const owner = conn.owner;
    const run = () => {
      const reply = this.command(owner, msg, r.id);
      if (reply) this.emit(conn, { ...reply, replica: this.ref(r) } as ServerMessage);
    };
    if (r.applied < conn.floor || r.waiting.length > 0) r.waiting.push({ floor: conn.floor, run });
    else run();
  }

  /**
   * Answers hello and resync with the replica's own applied state, held
   * until it has applied lastRevision (read index). A lastRevision above
   * the cluster's revision is answered at once with a snapshot.
   */
  private sync(conn: Conn, lastRevision: number | undefined, full = false): void {
    this.flush();
    const r = conn.replica;
    const floor = lastRevision !== undefined && lastRevision <= this.revision ? lastRevision : 0;
    if (r.applied < floor) {
      r.waiting.push({ floor, run: () => this.sync(conn, lastRevision, full) });
      return;
    }
    const replica = this.ref(r);
    if (lastRevision !== undefined && lastRevision > this.revision) {
      // The client holds a revision this cluster never committed.
      conn.synced = false;
      return this.emit(conn, { type: 'ahead', replica, revision: this.revision, lastRevision });
    }
    const you = conn.owner!;
    const revision = r.applied;
    conn.synced = true;
    if (!full && lastRevision !== undefined && lastRevision <= revision && lastRevision + 1 >= this.logStart) {
      const deltas = this.log
        .filter((e) => e.revision > lastRevision && e.revision <= revision)
        .map((e): DeltaMessage => ({ type: 'delta', replica, ...e }));
      return this.emit(conn, { type: 'resume', you, replica, revision, deltas });
    }
    this.emit(conn, {
      type: 'snapshot',
      you,
      replica,
      revision,
      shape: this.shape,
      state: r.view?.state ?? this.state,
      owners: r.view?.owners ?? this.owners,
    });
  }

  /**
   * Log entries reach the replica `ms` late from now on; 0 catches it up at
   * once. Dev control for read-index tests.
   */
  lag(id: string, ms: number): boolean {
    const r = this.replica(id);
    if (!r) return false;
    this.flush();
    if (ms > 0 && r.lagMs === 0) r.view = structuredClone({ state: this.state, owners: this.owners });
    r.lagMs = ms;
    if (ms === 0) {
      for (const e of this.log) if (e.revision > r.applied) this.deliver(r, e);
      r.applied = this.revision;
      r.view = null;
      this.drain(r);
    }
    return true;
  }

  /** One log entry reaches a replica: it applies it and sends it to its synced connections. */
  private deliver(r: Replica, e: LogEntry): void {
    if (e.revision <= r.applied) return;
    if (r.view) {
      for (const c of e.changes ?? []) setAt(r.view.state as unknown as Record<string, unknown>, c.path, structuredClone(c.value));
      for (const o of e.owners ?? []) {
        if (o.owner === null) delete r.view.owners[o.path];
        else r.view.owners[o.path] = o.owner;
      }
    }
    r.applied = e.revision;
    if (!r.partitioned) for (const c of r.conns) if (c.synced) this.emit(c, { type: 'delta', replica: this.ref(r), ...e });
    this.drain(r);
  }

  private drain(r: Replica): void {
    while (r.waiting.length > 0 && r.waiting[0]!.floor <= r.applied) r.waiting.shift()!.run();
  }

  // ---- commands -----------------------------------------------------------

  /**
   * Apply one WebSocket command. Returns the ack or error to send back
   * (without replica), or null when the delta carrying it answers it.
   */
  command(owner: Owner, msg: CommandMessage, replica: string): Omit<AckMessage, 'replica'> | Omit<ErrorMessage, 'replica'> | null {
    const audit = (result: AuditEntry['result'], revision: number) => {
      this.audit.push({ commandId: msg.id, owner, type: msg.type, path: msg.path, result, revision, replica });
      if (result !== 'applied') return;
      const set = msg.type === 'set';
      this.applies.push({
        owner,
        commandId: msg.id,
        type: msg.type,
        seq: set ? msg.seq : null,
        path: msg.path,
        value: set ? msg.value : null,
        revision,
        replica,
      });
    };
    const prior = this.applied.get(msg.id);
    if (prior !== undefined) {
      audit('duplicate', prior);
      return { type: 'ack', commandId: msg.id, result: 'duplicate', revision: prior };
    }
    const key = sessionKey(owner);
    const holder = this.owners[msg.path];
    const current = getAt(this.state, msg.path);
    if (msg.type !== 'release' && holder !== undefined && sessionKey(holder) !== key) {
      audit('conflict', this.revision);
      return {
        type: 'error',
        code: 409,
        commandId: msg.id,
        path: msg.path,
        current: current ?? null,
        owner: holder,
        message: `owned by ${holder.principal}/${holder.session}`,
      };
    }
    const fail = (code: 400 | 404, message: string) => ({ type: 'error' as const, code, commandId: msg.id, path: msg.path, message });
    const known = this.checkPath(msg.path);
    if (known) return fail(404, known);
    const done = (result: 'noop' | 'stale') => {
      this.applied.set(msg.id, this.revision);
      audit(result, this.revision);
      return { type: 'ack' as const, commandId: msg.id, result, revision: this.revision };
    };
    const origin: Origin = { owner, commandId: msg.id };
    if (msg.type === 'touch') {
      if (holder !== undefined) return done('noop');
      audit('applied', this.stage({ owners: [{ path: msg.path, owner }], origins: [origin] }).revision);
      return null;
    }
    if (msg.type === 'release') {
      if (holder === undefined || sessionKey(holder) !== key) return done('noop');
      audit('applied', this.stage({ changes: this.momentaryOff([msg.path]), owners: [{ path: msg.path, owner: null }], origins: [origin] }).revision);
      return null;
    }
    const seqKey = `${key}\n${msg.path}`;
    if (msg.seq <= (this.lastSeq.get(seqKey) ?? 0)) return done('stale');
    const invalid = this.checkValue(msg.path, msg.value);
    if (invalid) return fail(400, invalid);
    this.lastSeq.set(seqKey, msg.seq);
    if (equal(current, msg.value)) return done('noop');
    const staged = this.stage({ changes: this.withSideEffects([{ path: msg.path, value: msg.value }]), origins: [{ ...origin, seq: msg.seq }] });
    audit('applied', staged.revision);
    return null;
  }

  /** REST PATCH /state: all changes in one revision, or none. */
  patch(commandId: string, changes: Change[]): CommandResult {
    const prior = this.applied.get(commandId);
    if (prior !== undefined) return { commandId, result: 'duplicate', revision: prior };
    const conflicts: Conflict[] = changes
      .filter((c) => this.owners[c.path] !== undefined)
      .map((c) => ({ path: c.path, current: getAt(this.state, c.path) ?? null, owner: this.owners[c.path]! }));
    if (conflicts.length > 0) throw new CommandError(409, 'controls are owned by other sessions', conflicts);
    for (const c of changes) {
      const known = this.checkPath(c.path);
      if (known) throw new CommandError(404, known);
      const invalid = this.checkValue(c.path, c.value);
      if (invalid) throw new CommandError(400, invalid);
    }
    const effective = changes.filter((c) => !equal(getAt(this.state, c.path), c.value));
    if (effective.length === 0) {
      this.markApplied(commandId, 'noop');
      return { commandId, result: 'noop', revision: this.revision };
    }
    const staged = this.stage({ changes: this.withSideEffects(effective) });
    this.markApplied(commandId, 'applied', staged.revision);
    return { commandId, result: 'applied', revision: staged.revision, atIndex: staged.atIndex };
  }

  private checkPath(path: string): string | null {
    const [group, id] = parsePointer(path);
    if (group === 'recallSafe') return this.elementIds().has(id!) ? null : `unknown element ${id}`;
    if (group === 'monitor' || group === 'metering' || group === 'tone') return getAt(this.state, path) === undefined ? `unknown control ${path}` : null;
    return getAt(this.state, path) === undefined ? `unknown control ${path}` : null;
  }

  private checkValue(path: string, value: unknown): string | null {
    const channelIds = new Set(this.shape.channels.map((c) => c.id));
    if (/\/(exclude|members)$/.test(path)) {
      const bad = (value as string[]).find((id) => !channelIds.has(id));
      return bad ? `unknown channel ${bad}` : null;
    }
    const input = /^\/channels\/([^/]+)\/input$/.exec(path);
    if (input && value === 'b' && !this.shape.channels.find((c) => c.id === input[1])?.altInput) {
      return `channel ${input[1]} has no B input`;
    }
    if (path === '/tone/targets') {
      const outputs = new Set([...this.shape.buses, ...this.shape.auxes, ...this.shape.mixMinus].map((e) => e.id));
      const bad = (value as string[]).find((id) => !outputs.has(id));
      return bad ? `unknown output ${bad}` : null;
    }
    if (path === '/monitor/source') {
      const sources = new Set([...this.shape.buses, ...this.shape.auxes, ...this.shape.mixMinus].map((e) => e.id));
      return sources.has(value as string) ? null : `unknown monitor source ${String(value)}`;
    }
    return null;
  }

  private elementIds(): Set<string> {
    const s = this.shape;
    return new Set([...s.channels, ...s.buses, ...s.auxes, ...s.mixMinus, ...s.vcas].map((e) => e.id));
  }

  /**
   * Adds the changes that follow from the written ones: AFV stages from
   * binding or tally changes, loudness target and tolerance from a preset,
   * and preset custom from a written target or tolerance.
   */
  private withSideEffects(written: Change[]): Change[] {
    const changes = [...written];
    for (const c of written) {
      if (c.path === '/metering/loudness/preset' && c.value !== 'custom') {
        const p = LOUDNESS_PRESETS[c.value as 'ebu' | 'atsc'];
        changes.push({ path: '/metering/loudness/target', value: p.target }, { path: '/metering/loudness/tolerance', value: p.tolerance });
      }
      if ((c.path === '/metering/loudness/target' || c.path === '/metering/loudness/tolerance') && !written.some((w) => w.path === '/metering/loudness/preset')) {
        changes.push({ path: '/metering/loudness/preset', value: 'custom' });
      }
    }
    const next = structuredClone(this.state);
    for (const c of changes) setAt(next as unknown as Record<string, unknown>, c.path, c.value);
    const out = [...new Map(changes.map((c) => [c.path, c])).values()];
    for (const c of this.shape.channels) {
      const b = next.afv[c.id]!;
      let open = b.open;
      if (!b.enabled) open = true;
      else if (b.source !== null && next.tally[b.source] !== undefined) open = next.tally[b.source] === 'program';
      if (open !== this.state.afv[c.id]!.open) out.push({ path: pointer('afv', c.id, 'open'), value: open });
    }
    return out;
  }

  /**
   * Apply changes to the state now and add them to the log entry of the
   * current block, which is committed and fanned out BLOCK_MS later.
   */
  stage(entry: { changes?: Change[]; owners?: OwnershipChange[]; origins?: Origin[] }, atIndex?: number): { revision: number; atIndex?: number } {
    if (!this.batch) {
      this.batch = { revision: this.revision + 1, changes: [], owners: [], origins: [] };
      if (!this.holdBlocks) this.batchTimer = setTimeout(() => this.flush(), BLOCK_MS);
    }
    const b = this.batch;
    for (const c of entry.changes ?? []) {
      setAt(this.state as unknown as Record<string, unknown>, c.path, c.value);
      b.changes = b.changes.filter((x) => x.path !== c.path);
      b.changes.push(c);
    }
    if (entry.changes?.length) b.atIndex ??= atIndex ?? Math.floor(Date.now() * SAMPLES_PER_MS) + APPLY_MARGIN_SAMPLES;
    for (const o of entry.owners ?? []) {
      if (o.owner === null) delete this.owners[o.path];
      else this.owners[o.path] = o.owner;
      b.owners = b.owners.filter((x) => x.path !== o.path);
      b.owners.push(o);
    }
    for (const o of entry.origins ?? []) {
      b.origins.push(o);
      this.applied.set(o.commandId, b.revision);
    }
    return { revision: b.revision, atIndex: b.atIndex };
  }

  /** Commit the open log entry and send it to every synced connection. */
  flush(): void {
    const b = this.batch;
    if (!b) return;
    if (this.batchTimer) clearTimeout(this.batchTimer);
    this.batch = null;
    this.batchTimer = null;
    this.revision = b.revision;
    const e: LogEntry = { revision: b.revision };
    if (b.changes.length > 0) {
      e.atIndex = b.atIndex;
      e.changes = b.changes;
    }
    if (b.owners.length > 0) e.owners = b.owners;
    if (b.origins.length > 0) e.origins = b.origins;
    if (!e.changes && !e.owners) e.owners = [];
    if (e.owners?.length === 0) {
      // Every change in the block was undone within it; the revision still
      // advances so the command ids stay answered.
      e.changes = [{ path: '/scenes', value: this.state.scenes }];
      e.atIndex = b.atIndex ?? Math.floor(Date.now() * SAMPLES_PER_MS) + APPLY_MARGIN_SAMPLES;
      delete e.owners;
    }
    this.log.push(e);
    if (this.log.length > LOG_LIMIT) {
      this.log.shift();
      this.logStart++;
    }
    for (const r of this.replicas) {
      if (r.lagMs === 0) {
        this.deliver(r, e);
        continue;
      }
      const t = setTimeout(() => {
        this.lagTimers.delete(t);
        if (r.lagMs > 0) this.deliver(r, e);
      }, r.lagMs);
      this.lagTimers.add(t);
    }
  }

  /** true holds every processing block open; false ends the hold and flushes the open block. */
  holdBlock(hold: boolean): void {
    this.holdBlocks = hold;
    if (!hold) this.flush();
  }

  /** Advance the revision without telling clients; dev control for gap tests. */
  /**
   * Rolls the cluster back by `by` revisions, as a restore from an older
   * backup would; clients that saw later revisions are then ahead of it.
   */
  rewind(by: number): void {
    this.flush();
    this.revision = Math.max(0, this.revision - by);
    for (const r of this.replicas) r.applied = Math.min(r.applied, this.revision);
    this.log = this.log.filter((e) => e.revision <= this.revision);
    this.logStart = Math.min(this.logStart, this.revision + 1);
  }

  skipRevision(): void {
    this.flush();
    this.revision++;
    for (const r of this.replicas) {
      r.applied = this.revision;
      this.drain(r);
    }
    this.log = [];
    this.logStart = this.revision + 1;
  }

  // ---- scenes -------------------------------------------------------------

  private sceneData(): StoredScene['data'] {
    const s = structuredClone(this.state);
    return {
      channels: s.channels,
      buses: s.buses,
      auxes: s.auxes,
      mixMinus: s.mixMinus,
      vcas: s.vcas,
      afv: Object.fromEntries(Object.entries(s.afv).map(([id, { open: _open, ...b }]) => [id, b])),
    };
  }

  private publishScenes(): void {
    this.stage({ changes: [{ path: '/scenes', value: [...this.scenes.values()].map((s) => s.scene) }] });
  }

  storeScene(name: string, id: string | null): Scene {
    const sceneId = id ?? `scene-${++this.sceneCounter}`;
    const scene: Scene = { id: sceneId, name, storedAt: new Date().toISOString() };
    this.scenes.set(sceneId, { scene, data: this.sceneData() });
    this.publishScenes();
    return scene;
  }

  hasScene(id: string): boolean {
    return this.scenes.has(id);
  }

  sceneName(id: string): string {
    return this.scenes.get(id)!.scene.name;
  }

  deleteScene(id: string): void {
    this.scenes.delete(id);
    this.publishScenes();
  }

  /** Changes that recalling a scene makes, skipping recall-safe groups. */
  recallChanges(id: string): Change[] {
    const stored = this.scenes.get(id)!;
    const changes: Change[] = [];
    for (const [group, map] of Object.entries(stored.data)) {
      for (const [elementId, value] of Object.entries(map as Record<string, unknown>)) {
        for (const leaf of leaves(value, pointer(group, elementId))) {
          const g = recallGroup(leaf.path);
          if (g === null) continue;
          if (this.state.recallSafe[elementId]?.includes(g)) continue;
          if (!equal(getAt(this.state, leaf.path), leaf.value)) changes.push(leaf);
        }
      }
    }
    return changes;
  }

  recallScene(id: string): { revision: number; atIndex?: number; changed: boolean } {
    const changes = this.recallChanges(id);
    if (changes.length === 0) return { revision: this.revision, changed: false };
    return { ...this.stage({ changes: this.withSideEffects(changes) }), changed: true };
  }

  /** Record a REST command id as answered. */
  markApplied(commandId: string, result: 'applied' | 'noop' = 'applied', revision = this.batch?.revision ?? this.revision): void {
    this.applied.set(commandId, revision);
    this.audit.push({ commandId, owner: null, type: 'rest', path: null, result, revision, replica: 'rest' });
  }

  appliedAt(commandId: string): number | undefined {
    return this.applied.get(commandId);
  }

  /** Global revision a REST response reports: the open block's, if any. */
  get pendingRevision(): number {
    return this.batch?.revision ?? this.revision;
  }

  // ---- tally --------------------------------------------------------------

  applyTally(sources: Record<string, TallyState>, atIndex: number | null): number | undefined {
    const changes: Change[] = Object.entries(sources)
      .filter(([name, st]) => this.state.tally[name] !== st)
      .map(([name, st]) => ({ path: pointer('tally', name), value: st }));
    if (changes.length === 0) return undefined;
    return this.stage({ changes: this.withSideEffects(changes) }, atIndex ?? undefined).atIndex;
  }

  replaceTally(sources: Record<string, TallyState>): number | undefined {
    const all: Record<string, TallyState> = { ...sources };
    for (const name of Object.keys(this.state.tally)) if (!(name in all)) all[name] = 'off';
    return this.applyTally(all, null);
  }

  // ---- telemetry and heartbeats ----------------------------------------------

  private primaries(r: Replica): Conn[] {
    return [...r.conns].filter((c) => c.synced && c.mode === 'primary');
  }

  /**
   * One meter period: every serving replica counts it and sends its frame
   * to each synced primary connection holding a credit, consuming one.
   * A connection without credit misses the frame; nothing is queued.
   */
  private meterTick(): void {
    this.sources.advance();
    for (const r of this.replicas) {
      if (!this.ready(r)) continue;
      const sequence = r.meterSequence;
      r.meterSequence = (r.meterSequence + 1) >>> 0;
      const conns = this.primaries(r);
      if (conns.length === 0) continue;
      const values = r.meters.frame(this.state, this.sources);
      const receivers = conns.filter((c) => c.credit > 0);
      for (const c of conns) if (c.credit === 0) this.count(c, 'metersSkippedNoCredit');
      if (receivers.length === 0) continue;
      const bytes = encodeMeterFrame(this.meterLayout, { sequence, replica: r.id }, values);
      this.assertMeterFrame(bytes, sequence, r.id);
      for (const c of receivers) {
        c.credit--;
        this.count(c, 'meterFramesSent');
        this.count(c, 'messagesOut');
        c.send(bytes);
      }
    }
  }

  /** Decodes a frame about to be sent and validates its values against model.schema.json. */
  private assertMeterFrame(bytes: Uint8Array, sequence: number, replica: string): void {
    const frame = decodeMeterFrame(this.meterLayout, bytes);
    if (frame.sequence !== sequence || frame.replica !== replica) {
      throw new ContractViolation(`meter frame decodes to ${frame.replica}/${frame.sequence}, sent as ${replica}/${sequence}`);
    }
    this.contract.assertMeterValues(frame);
  }

  private counterTick(): void {
    for (const r of this.replicas) {
      if (!this.ready(r)) continue;
      if (Math.random() < 0.05) r.counters['input_late_blocks_total']! += 1;
      if (Math.random() < 0.02) r.counters['deadline_miss_total']! += 1;
      if (Math.random() < 0.03) r.counters['command_late_total']! += 1;
      for (const c of this.primaries(r)) this.emit(c, { type: 'counters', replica: this.ref(r), values: { ...r.counters } });
    }
  }

  /** Pings every connection after hello and closes those silent for DEAD_MS. */
  private heartbeatTick(): void {
    const t = Date.now();
    for (const r of this.replicas) {
      if (!this.ready(r)) continue;
      for (const c of [...r.conns]) {
        if (t - c.lastSeen > DEAD_MS) {
          c.close();
          continue;
        }
        if (c.owner) this.emit(c, { type: 'ping', replica: this.ref(r), t });
      }
    }
    this.expireLeases(t);
  }

  /** Release every control of a session whose lease ran out. */
  expireLeases(now = Date.now()): void {
    const released: OwnershipChange[] = Object.entries(this.owners)
      .filter(([, o]) => now - (this.leases.get(sessionKey(o)) ?? 0) > LEASE_MS)
      .map(([path]) => ({ path, owner: null }));
    if (released.length > 0) this.stage({ changes: this.momentaryOff(released.map((r) => r.path)), owners: released });
  }

  /** Momentary controls (IFB interrupt) among released paths go back to false. */
  private momentaryOff(paths: string[]): Change[] {
    return paths.filter((p) => /^\/mixMinus\/[^/]+\/interrupt\/active$/.test(p) && getAt(this.state, p) === true).map((path) => ({ path, value: false }));
  }

  resetLoudness(ids: string[]): void {
    for (const r of this.replicas) r.meters.resetLoudness(ids);
  }

  private broadcastReplicas(): void {
    const replicas = this.replicaStatus();
    for (const r of this.replicas) {
      for (const c of r.conns) if (c.synced) this.emit(c, { type: 'replicas', replica: this.ref(r), replicas });
    }
  }

  // ---- console simulation --------------------------------------------------

  private consoleBank(): void {
    const consoles = structuredClone(this.state.consoles);
    const mcu = consoles.find((c) => c.id === 'mcu-1')!;
    const ext = consoles.find((c) => c.id === 'ext-1')!;
    mcu.bankOffset = (mcu.bankOffset + 8) % Math.max(8, this.shape.channels.length - 8);
    ext.bankOffset = mcu.bankOffset + 8;
    const osc = consoles.find((c) => c.id === 'osc-1')!;
    osc.connected = !osc.connected;
    this.stage({ changes: [{ path: '/consoles', value: consoles }] });
  }

  /** A console operator rides a fader on the surface's second strip. */
  private consoleMotion(): void {
    const mcu = this.state.consoles.find((c) => c.id === 'mcu-1')!;
    const channel = this.shape.channels[mcu.bankOffset + 1];
    if (!channel) return;
    const path = pointer('channels', channel.id, 'fader');
    if (this.owners[path] !== undefined) return;
    const owner = CONSOLE_OWNER;
    const id = () => crypto.randomUUID();
    this.leases.set(sessionKey(owner), Date.now());
    if (this.command(owner, { type: 'touch', id: id(), path }, 'console')?.type === 'error') return;
    const start = this.state.channels[channel.id]!.fader;
    const base = start <= -60 ? -20 : start;
    let step = 0;
    let seq = Date.now();
    const timer = setInterval(() => {
      step++;
      this.leases.set(sessionKey(owner), Date.now());
      const value = Math.round((base + 4 * Math.sin((step / 20) * Math.PI)) * 10) / 10;
      this.command(owner, { type: 'set', id: id(), seq: ++seq, path, value }, 'console');
      if (step >= 20) {
        clearInterval(timer);
        this.command(owner, { type: 'set', id: id(), seq: ++seq, path, value: start }, 'console');
        this.command(owner, { type: 'release', id: id(), path }, 'console');
      }
    }, 100);
    this.timers.push(timer);
  }
}
