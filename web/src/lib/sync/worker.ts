// Entry of the sync worker: runs one SyncEngine on real WebSockets and
// performance.now(), fed by the page's messages.

import { performanceClock } from './clock';
import { SyncEngine, type SocketLike } from './engine';
import type { FromWorker, ToWorker } from './messages';

interface WorkerScope {
  onmessage: ((ev: MessageEvent<ToWorker>) => void) | null;
  postMessage(msg: FromWorker, transfer: Transferable[]): void;
}

const scope = self as unknown as WorkerScope;
let engine: SyncEngine | null = null;

scope.onmessage = (ev) => {
  const msg = ev.data;
  if (msg.type === 'init') {
    if (engine) return;
    engine = new SyncEngine({
      endpoints: msg.endpoints,
      session: msg.session,
      seqBase: msg.seqBase,
      socket: (url) => new WebSocket(url) as unknown as SocketLike,
      clock: performanceClock,
      post: (patch, transfer) => scope.postMessage(patch, transfer),
    });
    engine.start();
    return;
  }
  engine?.input(msg);
};
