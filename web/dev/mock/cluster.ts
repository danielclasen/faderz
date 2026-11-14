// Dev-only mock of a faderz replica set: one replicated state with a
// global revision, several replicas serving clients, touch ownership keyed
// by client id, idempotent command ids, scenes, AFV tally and console
// status. Every message it sends or receives is checked against the
// committed contract.

import type {
  AckMessage,
  Change,
  ClientMessage,
  CommandMessage,
  CommandResult,
  Conflict,
  DeltaMessage,
  ErrorMessage,
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
import { getAt, parsePointer, pointer, setAt } from '../../src/lib/state/pointer.ts';
import { Contract } from './contract.ts';
import { exampleShape, initialState } from './shape.ts';
import { ReplicaMeters, SourceModel } from './signals.ts';

export const OWNER_TIMEOUT_MS = 2000;
const LOG_LIMIT = 1000;
const SAMPLES_PER_MS = 48;
/** Scheduling margin between accepting a change and the sample it applies at. */
const APPLY_MARGIN_SAMPLES = 480;

type LogEntry = Omit<DeltaMessage, 'type' | 'replica'>;

export interface Conn {
  replica: Replica;
  clientId: string | null;
  synced: boolean;
  send(text: string): void;
  close(): void;
}

export interface Replica {
  id: string;
  role: ReplicaRole;
  healthy: boolean;
  /** none: serving; drop: hung, ignores input; apply: hung after replicating input. */
  frozen: 'none' | 'drop' | 'apply';
  conns: Set<Conn>;
  meters: ReplicaMeters;
  counters: Record<string, number>;
}

export interface AuditEntry {
  commandId: string;
  clientId: string;
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

function sampleIndexNow(): number {
  return Math.floor(Date.now() * SAMPLES_PER_MS) + APPLY_MARGIN_SAMPLES;
}

function equal(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Leaves of a state subtree; arrays of primitives count as one leaf. */
function leaves(value: unknown, base: string, out: Change[] = []): Change[] {
  if (Array.isArray(value) && value.every((v) => typeof v !== 'object' || v === null)) {
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
  if (group === 'mixMinus') return key === 'exclude' ? 'routing' : (key as RecallGroup);
  if (group === 'vcas') return key === 'offset' ? 'fader' : key === 'members' ? 'routing' : 'mute';
  switch (key) {
    case 'trim': case 'polarity': case 'hpf': case 'delay': return 'input';
    case 'eq': return 'eq';
    case 'gate': case 'comp': return 'dynamics';
    case 'pan': return 'pan';
    case 'sends': return 'sends';
    case 'routes': return 'routing';
    case 'fader': return 'fader';
    case 'mute': return 'mute';
    default: return null;
  }
}

interface StoredScene {
  scene: Scene;
  data: Pick<State, 'channels' | 'buses' | 'auxes' | 'mixMinus' | 'vcas'> & { afv: Record<string, { enabled: boolean; source: string | null }> };
}

export interface ClusterOptions {
  replicas?: number;
  automation?: boolean;
}

export class Cluster {
  readonly contract = new Contract();
  readonly shape: Shape;
  state: State;
  owners: Record<string, string> = {};
  revision = 0;
  readonly replicas: Replica[];
  readonly audit: AuditEntry[] = [];
  private log: LogEntry[] = [];
  private logStart = 1;
  private applied = new Map<string, number>();
  private lastSeq = new Map<string, number>();
  private scenes = new Map<string, StoredScene>();
  private sceneCounter = 0;
  private disconnectTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private timers: Array<ReturnType<typeof setInterval>> = [];
  private readonly sources: SourceModel;
  private readonly automation: boolean;

  constructor(options: ClusterOptions = {}) {
    this.shape = exampleShape();
    this.state = initialState(this.shape);
    this.sources = new SourceModel(this.shape);
    this.automation = options.automation ?? true;
    const n = options.replicas ?? 3;
    this.replicas = Array.from({ length: n }, (_, i) => ({
      id: `r${i + 1}`,
      role: i === 0 ? 'leader' : 'follower',
      healthy: true,
      frozen: 'none',
      conns: new Set<Conn>(),
      meters: new ReplicaMeters(this.shape),
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
  }

  // ---- lifecycle ----------------------------------------------------------

  start(): void {
    this.timers.push(setInterval(() => this.meterTick(), 40));
    this.timers.push(setInterval(() => this.counterTick(), 1000));
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
    for (const t of this.disconnectTimers.values()) clearTimeout(t);
    this.disconnectTimers.clear();
    for (const r of this.replicas) for (const c of r.conns) c.close();
  }

  ref(r: Replica): ReplicaRef {
    return { id: r.id, role: r.role };
  }

  replicaStatus(): ReplicaStatus[] {
    return this.replicas.map((r) => ({ id: r.id, role: r.role, healthy: r.healthy && r.frozen === 'none' }));
  }

  /** Replica a service endpoint routes a new connection to. */
  route(): Replica | null {
    const ready = this.replicas.filter((r) => r.healthy && r.frozen === 'none');
    if (ready.length === 0) return null;
    return ready.reduce((a, b) => (b.conns.size < a.conns.size ? b : a));
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

  restore(id: string): boolean {
    const r = this.replica(id);
    if (!r) return false;
    for (const c of [...r.conns]) if (r.frozen !== 'none') c.close();
    r.healthy = true;
    r.frozen = 'none';
    this.elect();
    this.broadcastReplicas();
    return true;
  }

  private elect(): void {
    const ready = (r: Replica) => r.healthy && r.frozen === 'none';
    const leader = this.replicas.find((r) => r.role === 'leader');
    if (leader && ready(leader)) return;
    const next = this.replicas.find(ready);
    if (!next) return;
    for (const r of this.replicas) r.role = r === next ? 'leader' : 'follower';
  }

  // ---- connections --------------------------------------------------------

  connect(replica: Replica, send: (text: string) => void, close: () => void): Conn {
    const conn: Conn = { replica, clientId: null, synced: false, send, close };
    replica.conns.add(conn);
    return conn;
  }

  disconnect(conn: Conn): void {
    conn.replica.conns.delete(conn);
    const clientId = conn.clientId;
    if (!clientId || this.connected(clientId)) return;
    this.disconnectTimers.set(
      clientId,
      setTimeout(() => {
        this.disconnectTimers.delete(clientId);
        if (this.connected(clientId)) return;
        const released: OwnershipChange[] = Object.entries(this.owners)
          .filter(([, owner]) => owner === clientId)
          .map(([path]) => ({ path, owner: null }));
        if (released.length > 0) this.commit({ owners: released });
      }, OWNER_TIMEOUT_MS),
    );
  }

  private connected(clientId: string): boolean {
    return this.replicas.some((r) => r.healthy && [...r.conns].some((c) => c.clientId === clientId));
  }

  emit(conn: Conn, msg: ServerMessage): void {
    if (conn.replica.frozen !== 'none' || !conn.replica.healthy) return;
    this.contract.assertServerMessage(msg);
    conn.send(JSON.stringify(msg));
  }

  receive(conn: Conn, text: string): void {
    if (!conn.replica.healthy || conn.replica.frozen === 'drop') return;
    let msg: ClientMessage;
    try {
      msg = JSON.parse(text) as ClientMessage;
    } catch {
      return this.emit(conn, { type: 'error', replica: this.ref(conn.replica), code: 400, message: 'not JSON' });
    }
    const problem = this.contract.checkClientMessage(msg);
    if (problem) {
      const commandId = (msg as { id?: unknown }).id;
      return this.emit(conn, {
        type: 'error',
        replica: this.ref(conn.replica),
        code: 400,
        message: `invalid message: ${problem}`,
        ...(typeof commandId === 'string' && /^[0-9a-f-]{36}$/i.test(commandId) ? { commandId } : {}),
      });
    }
    if (msg.type === 'hello') {
      conn.clientId = msg.clientId;
      const timer = this.disconnectTimers.get(msg.clientId);
      if (timer) clearTimeout(timer);
      this.disconnectTimers.delete(msg.clientId);
      this.emit(conn, { type: 'replicas', replica: this.ref(conn.replica), replicas: this.replicaStatus() });
      return this.sync(conn, msg.lastRevision);
    }
    if (!conn.clientId) {
      return this.emit(conn, { type: 'error', replica: this.ref(conn.replica), code: 400, message: 'hello first' });
    }
    if (msg.type === 'resync') return this.sync(conn, msg.lastRevision);
    const reply = this.command(conn.clientId, msg, conn.replica.id);
    if (reply) this.emit(conn, { ...reply, replica: this.ref(conn.replica) } as ServerMessage);
  }

  private sync(conn: Conn, lastRevision: number | undefined): void {
    const replica = this.ref(conn.replica);
    conn.synced = true;
    if (lastRevision !== undefined && lastRevision <= this.revision && lastRevision + 1 >= this.logStart) {
      const deltas = this.log
        .filter((e) => e.revision > lastRevision)
        .map((e): DeltaMessage => ({ type: 'delta', replica, ...e }));
      return this.emit(conn, { type: 'resume', clientId: conn.clientId!, replica, revision: this.revision, deltas });
    }
    this.emit(conn, {
      type: 'snapshot',
      clientId: conn.clientId!,
      replica,
      revision: this.revision,
      shape: this.shape,
      state: this.state,
      owners: this.owners,
    });
  }

  // ---- commands -----------------------------------------------------------

  /**
   * Apply one WebSocket command from `clientId`. Returns the ack or error to
   * send back (without replica), or null when the resulting delta answers it.
   */
  command(
    clientId: string,
    msg: CommandMessage,
    replica: string,
  ): Omit<AckMessage, 'replica'> | Omit<ErrorMessage, 'replica'> | null {
    const audit = (result: AuditEntry['result'], revision: number) =>
      this.audit.push({ commandId: msg.id, clientId, type: msg.type, path: msg.path, result, revision, replica });
    const prior = this.applied.get(msg.id);
    if (prior !== undefined) {
      audit('duplicate', prior);
      return { type: 'ack', commandId: msg.id, result: 'duplicate', revision: prior };
    }
    const owner = this.owners[msg.path];
    const current = getAt(this.state, msg.path);
    if (msg.type !== 'release' && owner !== undefined && owner !== clientId) {
      audit('conflict', this.revision);
      return {
        type: 'error',
        code: 409,
        commandId: msg.id,
        path: msg.path,
        current: current ?? null,
        owner,
        message: `owned by ${owner}`,
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
    const origin = { clientId, commandId: msg.id };
    if (msg.type === 'touch') {
      if (owner === clientId) return done('noop');
      this.commit({ owners: [{ path: msg.path, owner: clientId }], origin });
      audit('applied', this.revision);
      return null;
    }
    if (msg.type === 'release') {
      if (owner !== clientId) return done('noop');
      this.commit({ owners: [{ path: msg.path, owner: null }], origin });
      audit('applied', this.revision);
      return null;
    }
    const seqKey = `${clientId}\n${msg.path}`;
    if (msg.seq <= (this.lastSeq.get(seqKey) ?? 0)) return done('stale');
    const invalid = this.checkValue(msg.path, msg.value);
    if (invalid) return fail(400, invalid);
    this.lastSeq.set(seqKey, msg.seq);
    if (equal(current, msg.value)) return done('noop');
    this.commit({ changes: this.withSideEffects([{ path: msg.path, value: msg.value }]), origin: { ...origin, seq: msg.seq } });
    audit('applied', this.revision);
    return null;
  }

  /** REST PATCH /state: all changes in one revision, or none. */
  patch(commandId: string, changes: Change[]): CommandResult {
    const prior = this.applied.get(commandId);
    if (prior !== undefined) return { commandId, result: 'duplicate', revision: prior };
    const conflicts: Conflict[] = changes
      .filter((c) => this.owners[c.path] !== undefined)
      .map((c) => ({ path: c.path, current: getAt(this.state, c.path) ?? null, owner: this.owners[c.path]! }));
    if (conflicts.length > 0) throw new CommandError(409, 'controls are owned by other clients', conflicts);
    for (const c of changes) {
      const known = this.checkPath(c.path);
      if (known) throw new CommandError(404, known);
      const invalid = this.checkValue(c.path, c.value);
      if (invalid) throw new CommandError(400, invalid);
    }
    const effective = changes.filter((c) => !equal(getAt(this.state, c.path), c.value));
    const record = (result: AuditEntry['result']) =>
      this.audit.push({ commandId, clientId: 'rest', type: 'rest', path: null, result, revision: this.revision, replica: 'rest' });
    if (effective.length === 0) {
      this.applied.set(commandId, this.revision);
      record('noop');
      return { commandId, result: 'noop', revision: this.revision };
    }
    const atIndex = this.commit({ changes: this.withSideEffects(effective) });
    this.applied.set(commandId, this.revision);
    record('applied');
    return { commandId, result: 'applied', revision: this.revision, atIndex };
  }

  private checkPath(path: string): string | null {
    const [group, id] = parsePointer(path);
    if (group === 'recallSafe') return this.elementIds().has(id!) ? null : `unknown element ${id}`;
    if (group === 'monitor') return null;
    return getAt(this.state, path) === undefined ? `unknown control ${path}` : null;
  }

  private checkValue(path: string, value: unknown): string | null {
    const channelIds = new Set(this.shape.channels.map((c) => c.id));
    if (/\/(exclude|members)$/.test(path)) {
      const bad = (value as string[]).find((id) => !channelIds.has(id));
      return bad ? `unknown channel ${bad}` : null;
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

  /** Adds the AFV stage changes that follow from binding or tally changes. */
  private withSideEffects(changes: Change[]): Change[] {
    const next = structuredClone(this.state);
    for (const c of changes) setAt(next as unknown as Record<string, unknown>, c.path, c.value);
    const out = [...changes];
    for (const c of this.shape.channels) {
      const b = next.afv[c.id]!;
      let open = b.open;
      if (!b.enabled) open = true;
      else if (b.source !== null && next.tally[b.source] !== undefined) open = next.tally[b.source] === 'program';
      if (open !== this.state.afv[c.id]!.open) out.push({ path: pointer('afv', c.id, 'open'), value: open });
    }
    return out;
  }

  /** Append one change set to the replicated log and fan it out. Returns its atIndex. */
  commit(entry: Omit<LogEntry, 'revision' | 'atIndex'>, atIndex?: number): number | undefined {
    this.revision++;
    const e: LogEntry = { revision: this.revision, ...entry };
    if (entry.changes) {
      e.atIndex = atIndex ?? sampleIndexNow();
      for (const c of entry.changes) setAt(this.state as unknown as Record<string, unknown>, c.path, c.value);
    }
    if (entry.owners) {
      for (const o of entry.owners) {
        if (o.owner === null) delete this.owners[o.path];
        else this.owners[o.path] = o.owner;
      }
    }
    if (entry.origin) this.applied.set(entry.origin.commandId, this.revision);
    this.log.push(e);
    if (this.log.length > LOG_LIMIT) {
      this.log.shift();
      this.logStart++;
    }
    for (const r of this.replicas) {
      for (const c of r.conns) if (c.synced) this.emit(c, { type: 'delta', replica: this.ref(r), ...e });
    }
    return e.atIndex;
  }

  /** Advance the revision without telling clients; dev control for gap tests. */
  skipRevision(): void {
    this.revision++;
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
      afv: Object.fromEntries(Object.entries(s.afv).map(([id, b]) => [id, { enabled: b.enabled, source: b.source }])),
    };
  }

  private publishScenes(): void {
    const list = [...this.scenes.values()].map((s) => s.scene);
    this.commit({ changes: [{ path: '/scenes', value: list }] });
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
    const atIndex = this.commit({ changes: this.withSideEffects(changes) });
    return { revision: this.revision, atIndex, changed: true };
  }

  /** Record a REST command id as applied at the current revision. */
  markApplied(commandId: string): void {
    this.applied.set(commandId, this.revision);
    this.audit.push({ commandId, clientId: 'rest', type: 'rest', path: null, result: 'applied', revision: this.revision, replica: 'rest' });
  }

  appliedAt(commandId: string): number | undefined {
    return this.applied.get(commandId);
  }

  // ---- tally --------------------------------------------------------------

  applyTally(sources: Record<string, TallyState>, atIndex: number | null): number | undefined {
    const changes: Change[] = Object.entries(sources)
      .filter(([name, st]) => this.state.tally[name] !== st)
      .map(([name, st]) => ({ path: pointer('tally', name), value: st }));
    if (changes.length === 0) return undefined;
    return this.commit({ changes: this.withSideEffects(changes) }, atIndex ?? undefined);
  }

  replaceTally(sources: Record<string, TallyState>): number | undefined {
    const all: Record<string, TallyState> = { ...sources };
    for (const name of Object.keys(this.state.tally)) if (!(name in all)) all[name] = 'off';
    return this.applyTally(all, null);
  }

  // ---- telemetry ----------------------------------------------------------

  private meterTick(): void {
    this.sources.advance();
    for (const r of this.replicas) {
      if (!r.healthy || r.frozen !== 'none') continue;
      let frame: ReturnType<ReplicaMeters['frame']> | null = null;
      for (const c of r.conns) {
        if (!c.synced) continue;
        frame ??= r.meters.frame(this.state, this.sources);
        this.emit(c, { ...frame, replica: this.ref(r) });
      }
    }
  }

  private counterTick(): void {
    for (const r of this.replicas) {
      if (!r.healthy || r.frozen !== 'none') continue;
      if (Math.random() < 0.05) r.counters['input_late_blocks_total']! += 1;
      if (Math.random() < 0.02) r.counters['deadline_miss_total']! += 1;
      if (Math.random() < 0.03) r.counters['command_late_total']! += 1;
      for (const c of r.conns) if (c.synced) this.emit(c, { type: 'counters', replica: this.ref(r), values: { ...r.counters } });
    }
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
    mcu.bankOffset = (mcu.bankOffset + 8) % 24;
    ext.bankOffset = mcu.bankOffset + 8;
    const osc = consoles.find((c) => c.id === 'osc-1')!;
    osc.connected = !osc.connected;
    this.commit({ changes: [{ path: '/consoles', value: consoles }] });
  }

  /** A console operator rides a fader on the surface's first strip. */
  private consoleMotion(): void {
    const mcu = this.state.consoles.find((c) => c.id === 'mcu-1')!;
    const channel = this.shape.channels[mcu.bankOffset + 1];
    if (!channel) return;
    const path = pointer('channels', channel.id, 'fader');
    const clientId = 'mcu-1';
    if (this.owners[path] !== undefined) return;
    const id = () => crypto.randomUUID();
    if (this.command(clientId, { type: 'touch', id: id(), path }, 'console')?.type === 'error') return;
    const start = this.state.channels[channel.id]!.fader;
    const base = start <= -60 ? -20 : start;
    let step = 0;
    let seq = 0;
    const timer = setInterval(() => {
      step++;
      const value = Math.round((base + 4 * Math.sin((step / 20) * Math.PI)) * 10) / 10;
      this.command(clientId, { type: 'set', id: id(), seq: ++seq, path, value }, 'console');
      if (step >= 20) {
        clearInterval(timer);
        this.command(clientId, { type: 'set', id: id(), seq: ++seq, path, value: start }, 'console');
        this.command(clientId, { type: 'release', id: id(), path }, 'console');
      }
    }, 100);
    this.timers.push(timer);
  }
}
