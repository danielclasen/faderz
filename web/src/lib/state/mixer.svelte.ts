// Client-side replica of the mixer state. Applies snapshots, resumes and
// deltas in global revision order, keeps unanswered commands for resend
// after a failover and shows local values of in-flight writes so controls
// do not jump while their commands travel.

import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import { getContext } from 'svelte';
import type {
  ClientMessage,
  CommandMessage,
  DeltaMessage,
  Owners,
  ReplicaRef,
  ReplicaStatus,
  ServerMessage,
  Shape,
  State,
} from '../contract/types';
import { CommandLedger } from '../net/ledger';
import { uuid } from '../net/ids';
import { MeterStore } from '../meters/store';
import { getAt, setAt } from './pointer';
import { describePath } from './paths';

export type LinkState = 'connecting' | 'syncing' | 'live';

export interface Transport {
  send(msg: ClientMessage): boolean;
}

export interface Notice {
  id: number;
  kind: 'conflict' | 'error' | 'info';
  text: string;
}

export interface Failover {
  from: string;
  to: string;
  at: number;
}

export class MixerStore {
  readonly clientId: string;
  shape = $state.raw<Shape | null>(null);
  state = $state<State | null>(null);
  owners = $state<Owners>({});
  revision = $state(0);
  replica = $state.raw<ReplicaRef | null>(null);
  replicas = $state.raw<ReplicaStatus[]>([]);
  counters = $state.raw<Record<string, number>>({});
  countersReplica = $state<string | null>(null);
  meterReplica = $state<string | null>(null);
  link = $state<LinkState>('connecting');
  endpoint = $state<string | null>(null);
  lastSync = $state<'snapshot' | 'resume' | null>(null);
  failover = $state.raw<Failover | null>(null);
  notices = $state<Notice[]>([]);
  pendingCount = $state(0);
  /** Local value of every control with a write in flight. */
  readonly overlay = new SvelteMap<string, { value: unknown; commandId: string }>();
  /** Controls this client holds (pointer down or keyboard adjust). */
  readonly held = new SvelteSet<string>();
  /** Controls whose last write was refused because another client owns them. */
  readonly conflicts = new SvelteMap<string, number>();
  readonly meters = new MeterStore();

  private readonly ledger = new CommandLedger();
  private transport: Transport | null = null;
  private synced = false;
  private seq = 0;
  private servedBy: string | null = null;
  private noticeId = 0;

  constructor(clientId: string = uuid()) {
    this.clientId = clientId;
  }

  attach(transport: Transport): void {
    this.transport = transport;
  }

  // ---- reading -----------------------------------------------------------

  value<T>(path: string): T {
    const o = this.overlay.get(path);
    return (o ? o.value : getAt(this.state, path)) as T;
  }

  ownerOf(path: string): string | null {
    return this.owners[path] ?? null;
  }

  ownedByOther(path: string): boolean {
    const o = this.owners[path];
    return o !== undefined && o !== this.clientId;
  }

  // ---- writing -----------------------------------------------------------

  set(path: string, value: unknown): void {
    if (!this.state) return;
    const msg: CommandMessage = { type: 'set', id: uuid(), seq: ++this.seq, path, value };
    this.overlay.set(path, { value, commandId: msg.id });
    this.issue(msg);
  }

  touch(path: string): void {
    if (this.held.has(path)) return;
    this.held.add(path);
    if (this.owners[path] === this.clientId) return;
    this.issue({ type: 'touch', id: uuid(), path });
  }

  release(path: string): void {
    if (!this.held.delete(path)) return;
    this.issue({ type: 'release', id: uuid(), path });
  }

  private issue(msg: CommandMessage): void {
    this.ledger.add(msg);
    this.pendingCount = this.ledger.size;
    if (this.synced && this.transport?.send(msg)) this.ledger.markSent(msg.id);
  }

  // ---- link events -------------------------------------------------------

  opened(endpoint: string): void {
    this.endpoint = endpoint;
    this.link = 'syncing';
    this.synced = false;
    this.transport?.send(
      this.state
        ? { type: 'hello', clientId: this.clientId, lastRevision: this.revision }
        : { type: 'hello', clientId: this.clientId },
    );
  }

  closed(): void {
    this.synced = false;
    this.link = 'connecting';
  }

  receive(raw: unknown): void {
    const msg = (typeof raw === 'string' ? JSON.parse(raw) : raw) as ServerMessage;
    switch (msg.type) {
      case 'snapshot':
        this.shape = msg.shape;
        this.state = msg.state;
        this.owners = msg.owners;
        this.revision = msg.revision;
        this.serving(msg.replica);
        this.synchronised('snapshot');
        break;
      case 'resume':
        this.serving(msg.replica);
        for (const d of msg.deltas) {
          if (d.revision <= this.revision) continue;
          if (d.revision !== this.revision + 1) return this.resync();
          this.apply(d);
        }
        if (msg.revision !== this.revision) return this.resync();
        this.synchronised('resume');
        break;
      case 'delta':
        if (!this.synced || msg.revision <= this.revision) return;
        if (msg.revision !== this.revision + 1) return this.resync();
        this.apply(msg);
        if (this.replica?.role !== msg.replica.role) this.replica = msg.replica;
        break;
      case 'ack':
        this.resolve(msg.commandId);
        break;
      case 'error':
        if (msg.commandId) this.resolve(msg.commandId);
        if (msg.code === 409 && msg.path) {
          this.conflicts.set(msg.path, Date.now());
          this.notify(
            'conflict',
            `${describePath(msg.path, this.shape)} is held by ${msg.owner ?? 'another client'}`,
          );
        } else {
          this.notify('error', msg.message);
        }
        break;
      case 'replicas':
        this.replicas = msg.replicas;
        if (this.replica?.role !== msg.replica.role) this.replica = msg.replica;
        break;
      case 'meters':
        this.meters.ingest(msg, typeof performance === 'undefined' ? Date.now() : performance.now());
        if (this.meterReplica !== msg.replica.id) this.meterReplica = msg.replica.id;
        break;
      case 'counters':
        this.counters = msg.values;
        this.countersReplica = msg.replica.id;
        break;
    }
  }

  dismiss(id: number): void {
    this.notices = this.notices.filter((n) => n.id !== id);
  }

  notify(kind: Notice['kind'], text: string): void {
    this.notices = [...this.notices.slice(-4), { id: ++this.noticeId, kind, text }];
  }

  // ---- internals ---------------------------------------------------------

  private serving(replica: ReplicaRef): void {
    if (this.servedBy !== null && this.servedBy !== replica.id) {
      this.failover = { from: this.servedBy, to: replica.id, at: Date.now() };
      this.meters.clear();
    }
    this.servedBy = replica.id;
    this.replica = replica;
  }

  private synchronised(kind: 'snapshot' | 'resume'): void {
    this.synced = true;
    this.link = 'live';
    this.lastSync = kind;
    for (const msg of this.ledger.pending()) {
      if (this.transport?.send(msg)) this.ledger.markSent(msg.id);
    }
    // Ownership expires 2 s after a client loses every replica; take back
    // any control still held under the pointer.
    const touching = new Set(
      this.ledger.pending().flatMap((m) => (m.type === 'touch' ? [m.path] : [])),
    );
    for (const path of this.held) {
      if (this.owners[path] !== this.clientId && !touching.has(path)) {
        this.issue({ type: 'touch', id: uuid(), path });
      }
    }
  }

  private resync(): void {
    this.synced = false;
    this.link = 'syncing';
    this.transport?.send({ type: 'resync', lastRevision: this.revision });
  }

  private apply(d: DeltaMessage): void {
    if (d.changes && this.state) {
      for (const c of d.changes) setAt(this.state as unknown as Record<string, unknown>, c.path, c.value);
    }
    if (d.owners) {
      for (const o of d.owners) {
        if (o.owner === null) delete this.owners[o.path];
        else this.owners[o.path] = o.owner;
      }
    }
    this.revision = d.revision;
    if (d.origin?.clientId === this.clientId) this.resolve(d.origin.commandId);
  }

  private resolve(commandId: string): void {
    const msg = this.ledger.resolve(commandId);
    this.pendingCount = this.ledger.size;
    if (msg?.type === 'set' && this.overlay.get(msg.path)?.commandId === commandId) {
      this.overlay.delete(msg.path);
    }
  }
}

const KEY = Symbol('mixer');

/** Context map that makes `store` available to getMixer() below a mount. */
export function mixerContext(store: MixerStore): Map<symbol, MixerStore> {
  return new Map([[KEY, store]]);
}

export function getMixer(): MixerStore {
  return getContext<MixerStore>(KEY);
}
