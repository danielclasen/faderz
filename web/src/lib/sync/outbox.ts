// Commands awaiting their answer: a delta listing the command id among its
// origins, an ack or an error. Bounded and ordered. A set of a continuous
// control (a numeric value: fader, pan, send level, gain...) that has not
// been sent yet is replaced by a newer set of the same control; every
// other command keeps its place. After a failover every sent command is
// queued again with its id, which the server applies at most once.

import type { CommandId, CommandMessage } from '../contract/types';
import { COMMAND, TERMINAL, step, type CommandEvent, type CommandState } from './machines';

export const OUTBOX_CAPACITY = 1024;

export interface OutboxEntry {
  readonly msg: CommandMessage;
  readonly continuous: boolean;
  state: CommandState;
}

export type EnqueueResult =
  | { accepted: true; superseded: OutboxEntry | null }
  /** The outbox is full: the command is refused and never sent. */
  | { accepted: false };

export class Outbox {
  private entries: OutboxEntry[] = [];
  private setsByPath = new Map<string, number>();

  constructor(private readonly capacity = OUTBOX_CAPACITY) {}

  get size(): number {
    return this.entries.length;
  }

  /** True while a set of this control is unanswered. */
  hasSet(path: string): boolean {
    return this.setsByPath.has(path);
  }

  /** True while a command of this type for this control is unanswered. */
  has(type: CommandMessage['type'], path: string): boolean {
    return this.entries.some((e) => e.msg.type === type && e.msg.path === path);
  }

  enqueue(msg: CommandMessage, continuous: boolean): EnqueueResult {
    let superseded: OutboxEntry | null = null;
    if (msg.type === 'set' && continuous) {
      const last = this.lastFor(msg.path);
      if (last !== -1) {
        const e = this.entries[last]!;
        if (e.continuous && e.msg.type === 'set' && e.state === 'Queued') {
          e.state = step(COMMAND, e.state, 'supersede');
          this.entries.splice(last, 1);
          this.count(e.msg, -1);
          superseded = e;
        }
      }
    }
    if (this.entries.length >= this.capacity) return { accepted: false };
    this.entries.push({ msg, continuous, state: 'Queued' });
    this.count(msg, 1);
    return { accepted: true, superseded };
  }

  /** Queued commands in order; each is marked Sent. */
  transmit(): CommandMessage[] {
    const out: CommandMessage[] = [];
    for (const e of this.entries) {
      if (e.state !== 'Queued') continue;
      e.state = step(COMMAND, e.state, 'transmit');
      out.push(e.msg);
    }
    return out;
  }

  /** Apply an answer; a terminal command is removed and returned. */
  answer(id: CommandId, event: Extract<CommandEvent, 'ack' | 'reject' | 'stale'>): OutboxEntry | null {
    const i = this.entries.findIndex((e) => e.msg.id === id);
    if (i === -1) return null;
    const e = this.entries[i]!;
    e.state = step(COMMAND, e.state, event);
    if (!TERMINAL.has(e.state)) return null;
    this.entries.splice(i, 1);
    this.count(e.msg, -1);
    return e;
  }

  /** The connection failed: every sent command is queued again. Returns how many. */
  failover(): number {
    let n = 0;
    for (const e of this.entries) {
      const next = step(COMMAND, e.state, 'failover');
      if (next !== e.state) n++;
      e.state = next;
    }
    return n;
  }

  states(): Array<{ id: CommandId; state: CommandState }> {
    return this.entries.map((e) => ({ id: e.msg.id, state: e.state }));
  }

  private lastFor(path: string): number {
    for (let i = this.entries.length - 1; i >= 0; i--) if (this.entries[i]!.msg.path === path) return i;
    return -1;
  }

  private count(msg: CommandMessage, d: 1 | -1): void {
    if (msg.type !== 'set') return;
    const n = (this.setsByPath.get(msg.path) ?? 0) + d;
    if (n > 0) this.setsByPath.set(msg.path, n);
    else this.setsByPath.delete(msg.path);
  }
}
