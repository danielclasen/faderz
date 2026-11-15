// Commands awaiting their answer: a delta listing the command id among its
// origins, an ack or an error. Bounded and ordered. A set of a continuous
// control (contract/continuous.ts) that has not been sent yet is replaced
// by a newer set of the same control; every other command keeps its
// place. After a failover, a not-applied answer, or a snapshot answering
// a hello or resync it was sent before or answering none, a sent command is queued again
// with its id, which the server applies at most
// once; nothing else sends it again, as the server answers every command
// it reads, so a sent command counts in flight until it is answered or
// dropped. A command still unanswered at its expiry is
// dropped, so none is sent beyond the time the server remembers its id
// and seq. A command held after a not-applied answer can be overtaken by
// later commands of other controls, so the server may apply commands of
// different controls in another order than they were issued; seq keeps
// the commands of one control in order.

import type { CommandId, CommandMessage } from '../contract/types';
import { COMMAND, TERMINAL, step, type CommandEvent, type CommandState } from './machines';

export const OUTBOX_CAPACITY = 1024;

export interface OutboxEntry {
  readonly msg: CommandMessage;
  readonly continuous: boolean;
  state: CommandState;
  /** Clock time it was enqueued. */
  readonly issuedAt: number;
  /** Not sent again before this clock time (after a not-applied answer). */
  holdUntil: number;
  /** Sync generation it was last sent in (transmit's `generation`). */
  sentIn: number;
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

  /** Controls with an unanswered set of a discrete control (a key), each once, in order. */
  discreteSetPaths(): string[] {
    return [...new Set(this.entries.filter((e) => e.msg.type === 'set' && !e.continuous).map((e) => e.msg.path))];
  }

  /** Commands sent and not answered yet. */
  get inFlight(): number {
    return this.entries.filter((e) => e.state === 'Sent').length;
  }

  enqueue(msg: CommandMessage, continuous: boolean, now = 0): EnqueueResult {
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
    this.entries.push({ msg, continuous, state: 'Queued', issuedAt: now, holdUntil: -Infinity, sentIn: -Infinity });
    this.count(msg, 1);
    return { accepted: true, superseded };
  }

  /**
   * Queued commands in order, each marked Sent, until `limit` are in
   * flight; one held after a not-applied answer waits for its time, and
   * one issued at or before `issuedAfter` is past its expiry and never
   * sent. Each one sent is stamped with `generation`.
   */
  transmit(now = 0, limit = Infinity, issuedAfter = -Infinity, generation = 0): CommandMessage[] {
    const out: CommandMessage[] = [];
    let n = this.inFlight;
    for (const e of this.entries) {
      if (n >= limit) break;
      if (e.state !== 'Queued' || e.holdUntil > now || e.issuedAt <= issuedAfter) continue;
      e.state = step(COMMAND, e.state, 'transmit');
      e.sentIn = generation;
      out.push(e.msg);
      n++;
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

  /** Not applied (error 503): queued again, sent no earlier than `until`. */
  notApplied(id: CommandId, until: number): void {
    const e = this.entries.find((x) => x.msg.id === id);
    if (!e || e.state !== 'Sent') return;
    e.state = step(COMMAND, e.state, 'notApplied');
    e.holdUntil = until;
  }

  /** Drops every command enqueued at or before `before`; returns them in order. */
  expire(before: number): OutboxEntry[] {
    const out = this.entries.filter((e) => e.issuedAt <= before);
    if (out.length === 0) return out;
    this.entries = this.entries.filter((e) => e.issuedAt > before);
    for (const e of out) {
      e.state = step(COMMAND, e.state, 'expire');
      this.count(e.msg, -1);
    }
    return out;
  }

  /**
   * Every sent command is queued again (the connection failed), or only
   * those sent in a generation below `sentBefore`. Returns how many.
   */
  failover(sentBefore = Infinity): number {
    let n = 0;
    for (const e of this.entries) {
      if (e.sentIn >= sentBefore) continue;
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
