// Fake clock and sockets for the sync engine tests.

import type { Clock } from '../../src/lib/sync/clock';
import type { SocketLike } from '../../src/lib/sync/engine';
import type { ClientMessage, ServerMessage } from '../../src/lib/contract/types';

interface Timer {
  at: number;
  order: number;
  fn: () => void;
  cancelled: boolean;
}

export class FakeClock implements Clock {
  t = 0;
  private timers: Timer[] = [];
  private order = 0;

  now(): number {
    return this.t;
  }

  schedule(fn: () => void, ms: number): () => void {
    const timer = { at: this.t + Math.max(0, ms), order: this.order++, fn, cancelled: false };
    this.timers.push(timer);
    return () => {
      timer.cancelled = true;
    };
  }

  /**
   * The process stalls for `ms`: time moves on and no timer runs. Then the
   * timers that came due fire late, in time order, but not the ones they
   * schedule; messages queued during the stall come after them.
   */
  stall(ms: number): void {
    this.t += ms;
    const due = this.timers.filter((x) => !x.cancelled && x.at <= this.t).sort((a, b) => a.at - b.at || a.order - b.order);
    for (const x of due) {
      if (x.cancelled) continue;
      this.timers.splice(this.timers.indexOf(x), 1);
      x.fn();
    }
  }

  /** Advance time, running due timers in time order. */
  advance(ms: number): void {
    const end = this.t + ms;
    for (;;) {
      const due = this.timers
        .filter((x) => !x.cancelled && x.at <= end)
        .sort((a, b) => a.at - b.at || a.order - b.order)[0];
      if (!due) break;
      this.timers.splice(this.timers.indexOf(due), 1);
      this.t = due.at;
      due.fn();
    }
    this.t = end;
    this.timers = this.timers.filter((x) => !x.cancelled);
  }
}

export class FakeSocket implements SocketLike {
  binaryType = 'blob';
  sent: ClientMessage[] = [];
  closed = false;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  constructor(readonly url: string) {}

  send(data: string): void {
    this.sent.push(JSON.parse(data) as ClientMessage);
  }

  close(): void {
    this.closed = true;
  }

  // ---- server side ----

  open(): void {
    this.onopen?.({});
  }

  deliver(msg: ServerMessage | ArrayBuffer | Uint8Array): void {
    this.onmessage?.({ data: msg instanceof ArrayBuffer || msg instanceof Uint8Array ? msg : JSON.stringify(msg) });
  }

  /** The server drops the connection. */
  drop(): void {
    this.closed = true;
    this.onclose?.({});
  }

  ofType<T extends ClientMessage['type']>(type: T): Array<Extract<ClientMessage, { type: T }>> {
    return this.sent.filter((m): m is Extract<ClientMessage, { type: T }> => m.type === type);
  }
}
