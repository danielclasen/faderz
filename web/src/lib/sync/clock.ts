// Monotonic clock and timer scheduling, injected into the sync engine so it
// runs on performance.now() in the worker and on a fake clock in tests.

export interface Clock {
  /** Monotonic milliseconds. */
  now(): number;
  /** Run fn once after ms; returns a cancel function. */
  schedule(fn: () => void, ms: number): () => void;
}

export const performanceClock: Clock = {
  now: () => performance.now(),
  schedule: (fn, ms) => {
    const t = setTimeout(fn, Math.max(0, ms));
    return () => clearTimeout(t);
  },
};
