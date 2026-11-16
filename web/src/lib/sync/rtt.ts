// Round-trip time estimate of the link to the replicas, kept as TCP keeps
// it (RFC 6298): a smoothed mean and a mean deviation, each an EWMA. The
// sync engine sizes its deadlines from it, so they follow the link.

/** Samples above this are taken as a stalled clock, not a round trip. */
const MAX_SAMPLE_MS = 60_000;

export class RttEstimator {
  /** Smoothed round-trip time; null before the first sample. */
  srtt: number | null = null;
  /** Mean deviation of the samples from srtt. */
  rttvar = 0;

  sample(ms: number): void {
    if (!Number.isFinite(ms) || ms < 0 || ms > MAX_SAMPLE_MS) return;
    if (this.srtt === null) {
      this.srtt = ms;
      this.rttvar = ms / 2;
      return;
    }
    this.rttvar = 0.75 * this.rttvar + 0.25 * Math.abs(this.srtt - ms);
    this.srtt = 0.875 * this.srtt + 0.125 * ms;
  }

  /**
   * srtt + 4 rttvar: the time within which an answer is expected, TCP's
   * retransmission timeout without its 1 s floor. null before the first
   * sample.
   */
  get rto(): number | null {
    return this.srtt === null ? null : this.srtt + 4 * this.rttvar;
  }
}

/**
 * Exponential backoff: `base` doubled per failure, at most `cap`. With
 * `random` the delay is cut by up to a fifth, so clients that failed
 * together do not retry together; it never exceeds the unjittered value.
 */
export function backoff(base: number, failures: number, cap: number, random?: () => number): number {
  const d = Math.min(cap, base * 2 ** Math.max(0, failures));
  return random ? d * (1 - 0.2 * random()) : d;
}
