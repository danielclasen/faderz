// Page-side view of the mixer. The sync worker owns the connections and the
// replicated state; this store renders it. It applies the worker's patches
// leaf by leaf to the Svelte state proxy and forwards the operator's
// writes, touches and releases. A write shows its local value at once; the
// worker holds back server values of a control while a write of it is
// unanswered, and a patch built before the worker saw a write does not
// overwrite that write's value.

import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import { getContext } from 'svelte';
import type { Owner, Owners, Shape, State } from '../contract/types';
import { MeterStore } from '../meters/store';
import type { Patch, SyncStatus, ToWorker } from '../sync/messages';
import { handleTag } from '../net/handle';
import { getAt, setAt } from './pointer';
import { describePath } from './paths';
import { writesLockReason } from './writes';

export interface Port {
  post(msg: ToWorker): void;
}

export interface Notice {
  id: number;
  kind: 'conflict' | 'error' | 'info';
  text: string;
  /** Notices with one key (a control path) are one notice, counted. */
  key?: string;
  count: number;
}

export interface PatchStats {
  /** Duration of the last patch application, ms. */
  last: number;
  max: number;
  count: number;
}

export const INITIAL_STATUS: SyncStatus = {
  link: 'Connecting',
  standby: 'Idle',
  session: '',
  handle: '',
  you: null,
  primaryReplica: null,
  primaryEndpoint: null,
  standbyReplica: null,
  replicas: [],
  revision: 0,
  outbox: 0,
  failover: null,
  touching: [],
  lastSync: null,
  meterCredits: 0,
  ahead: null,
};

const NOTICE_LIMIT = 5;

export class MixerStore {
  shape = $state.raw<Shape | null>(null);
  state = $state<State | null>(null);
  owners = $state<Owners>({});
  status = $state.raw<SyncStatus>(INITIAL_STATUS);
  counters = $state.raw<Record<string, number>>({});
  countersReplica = $state<string | null>(null);
  meterReplica = $state<string | null>(null);
  notices = $state<Notice[]>([]);
  /** Page time (Date.now()) when the worker last reported a failover. */
  failoverAt = $state<number | null>(null);
  /** Controls under the pointer or a keyboard adjust. */
  readonly held = new SvelteSet<string>();
  /** Controls whose last write was refused because another session owns them. */
  readonly conflicts = new SvelteMap<string, number>();
  readonly meters = new MeterStore();
  readonly patchStats: PatchStats = { last: 0, max: 0, count: 0 };

  private port: Port | null = null;
  private posted = 0;
  /** Writes by page message number; a patch built before that message does not overwrite them. */
  private readonly localWrites = new Map<string, { input: number; value: unknown }>();
  private noticeId = 0;

  attach(port: Port): void {
    this.port = port;
  }

  get revision(): number {
    return this.status.revision;
  }

  get endpoint(): string | null {
    return this.status.primaryEndpoint;
  }

  get pendingCount(): number {
    return this.status.outbox;
  }

  /** FINE latch: faders, pans and parameters move at a tenth of their travel, as with Shift held. */
  fine = $state(false);
  /** The operator locked this screen: it shows everything and changes nothing. */
  deskLock = $state(false);
  /** Why write controls are disabled and send nothing (link not transmitting, or no quorum and no ready standby), or null. */
  readonly lockReason = $derived(writesLockReason(this.status));
  /** Write controls are disabled and send nothing: for lockReason, or the desk is locked. */
  readonly locked = $derived(this.lockReason !== null || this.deskLock);

  // ---- reading -----------------------------------------------------------

  value<T>(path: string): T {
    return getAt(this.state, path) as T;
  }

  ownerOf(path: string): Owner | null {
    return this.owners[path] ?? null;
  }

  isMine(owner: Owner | null): boolean {
    const you = this.status.you;
    if (!owner) return false;
    if (you) return owner.principal === you.principal && owner.handle === you.handle;
    return owner.handle === this.status.handle;
  }

  ownedByOther(path: string): boolean {
    const o = this.ownerOf(path);
    return o !== null && !this.isMine(o);
  }

  /** Display name of an owner: "you", a console's name, or kind and handle tag. */
  ownerLabel(owner: Owner | null): string {
    if (!owner) return 'another session';
    if (this.isMine(owner)) return 'you';
    const principal = owner.principal === 'anonymous' ? '' : `${owner.principal} `;
    if (owner.kind === 'console') {
      const c = this.state?.consoles.find((x) => x.handle === owner.handle);
      if (c) return c.name;
    }
    const kind = owner.kind === 'ui' ? 'web client' : owner.kind;
    return `${principal}${kind} ${handleTag(owner.handle)}`;
  }

  // ---- writing -----------------------------------------------------------

  set(path: string, value: unknown): void {
    if (!this.state || this.locked) return;
    setAt(this.state as unknown as Record<string, unknown>, path, value);
    this.post({ type: 'set', path, value });
    this.localWrites.set(path, { input: this.posted, value });
  }

  touch(path: string): void {
    if (this.locked || this.held.has(path)) return;
    this.held.add(path);
    this.post({ type: 'touch', path });
  }

  /**
   * A gesture on a control another session holds: nothing is sent, the
   * control flashes and the notice names the holder, at once rather than
   * after a refused round trip. Returns true when the control is held.
   */
  refuseIfHeld(path: string): boolean {
    const owner = this.ownerOf(path);
    if (owner === null || this.isMine(owner)) return false;
    this.conflicts.set(path, Date.now());
    this.notify('conflict', `${describePath(path, this.shape)} is held by ${this.ownerLabel(owner)}`, path);
    return true;
  }

  /** Sent also while locked: it ends a touch made before, and changes no value. */
  release(path: string): void {
    if (!this.held.delete(path)) return;
    this.post({ type: 'release', path });
  }

  /** Takes the cluster state while this client is ahead of it. */
  adopt(): void {
    this.post({ type: 'adopt' });
  }

  /** Called once per animation frame while the page is visible. */
  tick(): void {
    this.post({ type: 'tick' });
  }

  private post(msg: ToWorker): void {
    this.posted++;
    this.port?.post(msg);
  }

  // ---- worker patches ------------------------------------------------------

  apply(p: Patch, now: number = performance.now()): void {
    performance.mark('faderz:patch-start');
    const fresh = (path: string) => (this.localWrites.get(path)?.input ?? 0) <= p.input;
    if (p.snapshot) {
      this.shape = p.snapshot.shape;
      this.state = p.snapshot.state;
      this.owners = p.snapshot.owners;
      this.meters.setShape(p.snapshot.shape);
      for (const [path, w] of this.localWrites) {
        if (!fresh(path)) setAt(this.state as unknown as Record<string, unknown>, path, w.value);
      }
    }
    const state = this.state as unknown as Record<string, unknown> | null;
    if (state) {
      for (const [path, value] of p.changes) {
        if (!fresh(path) || Object.is(getAt(state, path), value)) continue;
        setAt(state, path, value);
      }
    }
    for (const [path, owner] of p.owners) {
      if (owner === null) delete this.owners[path];
      else this.owners[path] = owner;
    }
    for (const [path, w] of this.localWrites) if (w.input <= p.input) this.localWrites.delete(path);
    if (p.status) {
      if (p.status.failover && p.status.failover.n !== this.status.failover?.n) this.failoverAt = Date.now();
      this.status = p.status;
    }
    if (p.meters) {
      this.meters.ingest(p.meters.replica, p.meters.values, now);
      if (this.meterReplica !== p.meters.replica) this.meterReplica = p.meters.replica;
    }
    if (p.counters) {
      this.counters = p.counters.values;
      this.countersReplica = p.counters.replica;
    }
    for (const n of p.notices ?? []) {
      if (n.kind === 'conflict') {
        this.conflicts.set(n.path, Date.now());
        this.notify('conflict', `${describePath(n.path, this.shape)} is held by ${this.ownerLabel(n.owner)}`, n.path);
      } else if (n.kind === 'expired') {
        this.notify('info', `Not confirmed in time, not sent again: ${n.paths.map((p) => describePath(p, this.shape)).join(', ')}`);
      } else {
        this.notify('error', n.text);
      }
    }
    performance.clearMeasures('faderz:patch');
    const m = performance.measure('faderz:patch', 'faderz:patch-start');
    performance.clearMarks('faderz:patch-start');
    const s = this.patchStats;
    s.last = m.duration;
    s.max = Math.max(s.max, m.duration);
    s.count++;
  }

  dismiss(id: number): void {
    this.notices = this.notices.filter((n) => n.id !== id);
  }

  /** Adds a notice; one with the key of a shown notice replaces it and counts up. */
  notify(kind: Notice['kind'], text: string, key?: string): void {
    const same = key === undefined ? undefined : this.notices.find((n) => n.key === key);
    const rest = this.notices.filter((n) => n !== same).slice(-(NOTICE_LIMIT - 1));
    this.notices = [...rest, { id: ++this.noticeId, kind, text, key, count: (same?.count ?? 0) + 1 }];
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
