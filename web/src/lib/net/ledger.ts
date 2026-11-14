// Commands awaiting their answer (a delta carrying the command id, an ack
// or an error). Unanswered commands are resent with the same id after a
// reconnect; the server applies a command id at most once.

import type { CommandId, CommandMessage } from '../contract/types';

interface Entry {
  msg: CommandMessage;
  sent: boolean;
}

export class CommandLedger {
  private entries = new Map<CommandId, Entry>();

  /**
   * Record a command. An unsent set for the same control is superseded and
   * dropped: it never reached a server, and the newer set carries the value.
   */
  add(msg: CommandMessage): void {
    if (msg.type === 'set') {
      for (const [id, e] of this.entries) {
        if (!e.sent && e.msg.type === 'set' && e.msg.path === msg.path) this.entries.delete(id);
      }
    }
    this.entries.set(msg.id, { msg, sent: false });
  }

  markSent(id: CommandId): void {
    const e = this.entries.get(id);
    if (e) e.sent = true;
  }

  /** Remove an answered command; returns it, or undefined if unknown. */
  resolve(id: CommandId): CommandMessage | undefined {
    const e = this.entries.get(id);
    if (!e) return undefined;
    this.entries.delete(id);
    return e.msg;
  }

  has(id: CommandId): boolean {
    return this.entries.has(id);
  }

  /** Every unanswered command in the order it was issued. */
  pending(): CommandMessage[] {
    return [...this.entries.values()].map((e) => e.msg);
  }

  get size(): number {
    return this.entries.size;
  }
}
