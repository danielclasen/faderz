// Engine counters are totals since the engine started. An operator needs
// to know whether something is going wrong now, so the lamp follows the
// counters that grew within the last window, not their totals.

import { fit } from './format';

export const WARN_WINDOW_MS = 10_000;

/** Width of the lamp text: "ENGINE WARN 99+" is the longest. */
const LAMP_W = 15;

/** The lamp's text for `growing` counters, at one width for every count. */
export function engineLamp(growing: number): string {
  return fit(growing === 0 ? 'ENGINE OK' : `ENGINE WARN ${growing > 99 ? '99+' : growing}`, LAMP_W);
}

export class CounterRates {
  /** Last value of each counter and when it last grew. */
  private readonly seen = new Map<string, { value: number; grewAt: number }>();
  private replica: string | null = null;

  /**
   * Names of the counters of `replica` that grew within WARN_WINDOW_MS
   * before `now`, sorted. Totals of another replica are a new baseline:
   * after a failover they are another engine's, not growth.
   */
  update(values: Record<string, number>, now: number, replica: string | null = null): string[] {
    if (replica !== this.replica) {
      this.seen.clear();
      this.replica = replica;
    }
    for (const [name, value] of Object.entries(values)) {
      const prev = this.seen.get(name);
      // The first reading is a baseline: a total from before this screen opened is not "now".
      if (!prev) this.seen.set(name, { value, grewAt: -Infinity });
      else if (value > prev.value) this.seen.set(name, { value, grewAt: now });
      // A counter that went down was reset (engine restart).
      else if (value < prev.value) this.seen.set(name, { value, grewAt: prev.grewAt });
    }
    return [...this.seen.entries()]
      .filter(([name, s]) => name in values && now - s.grewAt <= WARN_WINDOW_MS)
      .map(([name]) => name)
      .sort();
  }
}
