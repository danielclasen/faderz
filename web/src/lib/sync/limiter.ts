// Per-control write rate limit: at most MAX_WRITES_PER_SECOND writes per
// control. A write inside the interval is held as the control's trailing
// value, replaced by later writes, and emitted when the interval ends, so
// the latest value always goes out.

import { MAX_WRITES_PER_SECOND } from '../contract/protocol';
import type { Clock } from './clock';

export const WRITE_INTERVAL_MS = 1000 / MAX_WRITES_PER_SECOND;

export class RateLimiter {
  private last = new Map<string, number>();
  private trailing = new Map<string, unknown>();

  constructor(
    private readonly clock: Clock,
    private readonly emit: (path: string, value: unknown) => void,
  ) {}

  /** Trailing values waiting for their slot. */
  get size(): number {
    return this.trailing.size;
  }

  /** True while a trailing value of this control waits for its slot. */
  holds(path: string): boolean {
    return this.trailing.has(path);
  }

  write(path: string, value: unknown): void {
    if (this.trailing.has(path)) {
      this.trailing.set(path, value);
      return;
    }
    const now = this.clock.now();
    const last = this.last.get(path);
    if (last === undefined || now - last >= WRITE_INTERVAL_MS) {
      this.last.set(path, now);
      this.emit(path, value);
      return;
    }
    this.trailing.set(path, value);
    this.clock.schedule(() => {
      const v = this.trailing.get(path);
      this.trailing.delete(path);
      this.last.set(path, this.clock.now());
      this.emit(path, v);
    }, last + WRITE_INTERVAL_MS - now);
  }
}
