// Page side of the sync worker: starts it, forwards the store's messages,
// sends a tick per animation frame while the page is visible, applies the
// patches that answer the ticks, and exposes window.__faderz, a read-only
// view of the sync state for tests and debugging.

import { onAnimationFrame } from '../meters/store';
import type { MixerStore } from '../state/mixer.svelte';
import type { FromWorker, ToWorker } from './messages';
import { pageSession } from './session';

export interface FaderzDebug {
  readonly connection: string;
  readonly standby: string;
  readonly primaryReplica: string | null;
  readonly standbyReplica: string | null;
  /**
   * Dev builds only: the session id is a credential, and a production
   * build gives no script on the page a way to read it.
   */
  readonly session: string;
  readonly revision: number;
  readonly outbox: number;
  readonly failovers: number;
  readonly lastFailover: { from: string | null; to: string | null; reason: string } | null;
  readonly replicas: ReadonlyArray<{ id: string; role: string; healthy: boolean; commitable: boolean }>;
  readonly touching: readonly string[];
  readonly meterReplica: string | null;
  readonly meterCredits: number;
  /** True while the page sends ticks (visible). */
  readonly ticking: boolean;
  readonly lastPatchMs: number;
  readonly maxPatchMs: number;
  readonly patches: number;
}

declare global {
  interface Window {
    __faderz?: FaderzDebug;
  }
}

export interface SyncHandle {
  /** Hands the worker a round trip the page measured to the server, in ms. */
  rtt(ms: number): void;
}

export function startSync(store: MixerStore, endpoints: string[]): SyncHandle {
  const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
  worker.onmessage = (ev: MessageEvent<FromWorker>) => store.apply(ev.data);
  worker.onerror = (ev) => store.notify('error', `Sync worker failed: ${ev.message}`);
  store.attach({ post: (msg: ToWorker) => worker.postMessage(msg) });
  const init: ToWorker = { type: 'init', endpoints, session: pageSession(), seqBase: Date.now() };
  worker.postMessage(init);

  let stop: (() => void) | null = null;
  const follow = () => {
    if (document.hidden) {
      stop?.();
      stop = null;
    } else {
      stop ??= onAnimationFrame(() => store.tick());
    }
  };
  document.addEventListener('visibilitychange', follow);
  follow();
  exposeDebug(store, () => stop !== null);
  return { rtt: (ms) => worker.postMessage({ type: 'rtt', ms } satisfies ToWorker) };
}

function exposeDebug(store: MixerStore, ticking: () => boolean): void {
  const s = () => store.status;
  const view: Omit<FaderzDebug, 'session'> = {
    get connection() { return s().link; },
    get standby() { return s().standby; },
    get primaryReplica() { return s().primaryReplica; },
    get standbyReplica() { return s().standbyReplica; },
    get revision() { return s().revision; },
    get outbox() { return s().outbox; },
    get failovers() { return s().failover?.n ?? 0; },
    get lastFailover() {
      const f = s().failover;
      return f ? { from: f.from, to: f.to, reason: f.reason } : null;
    },
    get replicas() { return s().replicas.map((r) => ({ ...r })); },
    get touching() { return [...s().touching]; },
    get meterReplica() { return store.meterReplica; },
    get meterCredits() { return s().meterCredits; },
    get ticking() { return ticking(); },
    get lastPatchMs() { return store.patchStats.last; },
    get maxPatchMs() { return store.patchStats.max; },
    get patches() { return store.patchStats.count; },
  };
  if (import.meta.env.DEV) Object.defineProperty(view, 'session', { get: () => s().session, enumerable: true });
  Object.defineProperty(window, '__faderz', { value: Object.freeze(view) as FaderzDebug, enumerable: true });
}
