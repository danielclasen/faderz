// Display ballistics for meters fed at 25 Hz.

export const METER_FLOOR_DB = -60;
export const PEAK_HOLD_MS = 1500;
/**
 * Fall rate of the bar, dB per second: 20 dB in 1.7 s, the return time of
 * an IEC 60268-18 digital peak level meter.
 */
export const BAR_FALL_DB_PER_S = 20 / 1.7;
/** Fall rate of the peak-hold marker after the hold time, dB per second. */
export const HOLD_FALL_DB_PER_S = 12;

/**
 * Meter scale as [dBFS, position] pairs. Resolution is highest between
 * -20 dBFS and 0 dBFS, where alignment and permitted maximum sit.
 */
export const METER_SCALE: ReadonlyArray<readonly [number, number]> = [
  [-60, 0],
  [-50, 0.07],
  [-40, 0.16],
  [-30, 0.3],
  [-20, 0.48],
  [-10, 0.7],
  [-6, 0.8],
  [0, 1],
];
/** Marked levels of a meter scale: full scale, the zone boundaries and every 10 dB below -20. */
export function meterMarks(zones: { alignment: number; permittedMax: number }): number[] {
  return [...new Set([0, zones.permittedMax, zones.alignment, -30, -40, -50, -60])]
    .filter((m) => m <= 0 && m >= METER_FLOOR_DB)
    .sort((a, b) => b - a);
}

export function meterPosition(db: number): number {
  if (db <= METER_SCALE[0]![0]) return 0;
  for (let i = 1; i < METER_SCALE.length; i++) {
    const a = METER_SCALE[i - 1]!;
    const b = METER_SCALE[i]!;
    if (db <= b[0]) return a[1] + ((db - a[0]) / (b[0] - a[0])) * (b[1] - a[1]);
  }
  return 1;
}

export interface LegBallistics {
  bar: number;
  hold: number;
  holdSince: number;
}

export function newLeg(): LegBallistics {
  return { bar: METER_FLOOR_DB, hold: METER_FLOOR_DB, holdSince: 0 };
}

/**
 * Advance one meter leg to time `now` (ms) given the latest frame value.
 * The bar rises instantly and falls at BAR_FALL_DB_PER_S; the hold marker
 * keeps the highest value for PEAK_HOLD_MS, then falls at HOLD_FALL_DB_PER_S.
 */
export function stepLeg(leg: LegBallistics, input: number, now: number, dtMs: number): void {
  const value = Math.max(input, METER_FLOOR_DB);
  const fallen = leg.bar - (BAR_FALL_DB_PER_S * dtMs) / 1000;
  leg.bar = Math.max(value, fallen, METER_FLOOR_DB);
  if (value >= leg.hold) {
    leg.hold = value;
    leg.holdSince = now;
  } else if (now - leg.holdSince > PEAK_HOLD_MS) {
    leg.hold = Math.max(leg.hold - (HOLD_FALL_DB_PER_S * dtMs) / 1000, leg.bar, METER_FLOOR_DB);
  }
}
