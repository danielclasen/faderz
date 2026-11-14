// Fader taper, gain conversion and dB formatting.

/** Fader floor in the contract: -90 dB means fully closed (-inf). */
export const FADER_OFF_DB = -90;
export const FADER_MAX_DB = 10;
/** Effective channel gain (fader plus VCA offsets) is clamped here. */
export const EFFECTIVE_MAX_DB = 20;

/**
 * Fader travel as [position, dB] pairs, position 0 at the bottom and 1 at
 * the top. 0 dB sits at three quarters of the travel, the usual broadcast
 * console scale; linear in dB between the points.
 */
export const FADER_TAPER: ReadonlyArray<readonly [number, number]> = [
  [0, FADER_OFF_DB],
  [0.075, -60],
  [0.15, -40],
  [0.25, -30],
  [0.375, -20],
  [0.5, -10],
  [0.625, -5],
  [0.75, 0],
  [0.875, 5],
  [1, FADER_MAX_DB],
];

/** dB marks drawn beside a fader. */
export const FADER_MARKS: readonly number[] = [10, 5, 0, -5, -10, -20, -30, -40, -60, FADER_OFF_DB];

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

function interpolate(
  table: ReadonlyArray<readonly [number, number]>,
  x: number,
  from: 0 | 1,
  to: 0 | 1,
): number {
  const first = table[0]!;
  const last = table[table.length - 1]!;
  if (x <= first[from]) return first[to];
  if (x >= last[from]) return last[to];
  for (let i = 1; i < table.length; i++) {
    const a = table[i - 1]!;
    const b = table[i]!;
    if (x <= b[from]) {
      const t = (x - a[from]) / (b[from] - a[from]);
      return a[to] + t * (b[to] - a[to]);
    }
  }
  return last[to];
}

/** Fader position (0..1) for a dB value. */
export function dbToPosition(db: number): number {
  return interpolate(FADER_TAPER, clamp(db, FADER_OFF_DB, FADER_MAX_DB), 1, 0);
}

/** dB value for a fader position (0..1), rounded to 0.1 dB; the floor snaps to off. */
export function positionToDb(pos: number): number {
  const db = interpolate(FADER_TAPER, clamp(pos, 0, 1), 0, 1);
  if (db <= FADER_OFF_DB + 0.05) return FADER_OFF_DB;
  return roundDb(db);
}

export function roundDb(db: number): number {
  const r = Math.round(db * 10) / 10;
  return Object.is(r, -0) ? 0 : r;
}

/** Linear gain for a dB value; the fader floor is silence. */
export function dbToGain(db: number): number {
  return db <= FADER_OFF_DB ? 0 : Math.pow(10, db / 20);
}

export function gainToDb(gain: number): number {
  return gain <= 0 ? -Infinity : 20 * Math.log10(gain);
}

/**
 * Effective channel gain in dB: fader plus the sum of the VCA offsets of
 * every membership, clamped at +20 dB. A closed fader or a closed VCA keeps
 * the channel closed.
 */
export function effectiveGainDb(fader: number, vcaOffsets: readonly number[]): number {
  if (fader <= FADER_OFF_DB) return FADER_OFF_DB;
  let sum = fader;
  for (const o of vcaOffsets) {
    if (o <= FADER_OFF_DB) return FADER_OFF_DB;
    sum += o;
  }
  return Math.min(sum, EFFECTIVE_MAX_DB);
}

/** Fader dB as shown on a strip: "-inf", "0.0", "+3.5", "-12.0". */
export function formatDb(db: number): string {
  if (db <= FADER_OFF_DB) return '-inf';
  const r = roundDb(db);
  return r > 0 ? `+${r.toFixed(1)}` : r.toFixed(1);
}

/**
 * Step a fader value by `step` dB, as keyboard control does. Stepping up
 * from off lands on -60 dB, the lowest marked level.
 */
/**
 * Keyboard step at a fader value, following the taper: 0.5 dB from -20 dB
 * up, where the most travel is, 1 dB down to -40 dB and 3 dB below.
 */
export function keyStepDb(db: number): number {
  return db > -20 ? 0.5 : db > -40 ? 1 : 3;
}

export function stepDb(db: number, step: number): number {
  if (db <= FADER_OFF_DB) return step > 0 ? -60 : FADER_OFF_DB;
  const next = roundDb(db + step);
  if (next < -60 && step < 0) return FADER_OFF_DB;
  return clamp(next, FADER_OFF_DB, FADER_MAX_DB);
}

/**
 * New fader value after a pointer drag of `dy` pixels (positive is down)
 * on a track `height` pixels tall. `fine` divides the travel by ten.
 */
export function dragDb(startDb: number, dy: number, height: number, fine: boolean): number {
  const scale = fine ? 0.1 : 1;
  const pos = dbToPosition(startDb) - (dy / height) * scale;
  return positionToDb(pos);
}

/** Pan value as text: "C", "L42", "R100". */
export function formatPan(v: number, left = 'L', right = 'R'): string {
  const p = Math.round(v * 100);
  if (p === 0) return 'C';
  return p < 0 ? `${left}${-p}` : `${right}${p}`;
}

export function formatFreq(hz: number): string {
  return hz >= 1000 ? `${(hz / 1000).toFixed(hz >= 10000 ? 1 : 2)}k` : `${Math.round(hz)}`;
}
