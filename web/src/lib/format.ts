// Readouts whose text changes keep one width: padded to a fixed number of
// characters, shown in the tabular monospace face with white-space: pre
// (class "fixed" in app.css). A right-aligned or centred text of another
// length would start elsewhere, which is a layout shift.

/** `text` padded at the start to `width` characters. */
export function fit(text: string, width: number): string {
  return text.padStart(width);
}

/** Lowest level read as a number; anything at or below it reads "-inf". */
const INF_DB = -100;

/**
 * A loudness or level readout in five characters, one decimal place at
 * most (EBU Tech 3341): "--.-" without a value, "-inf" at or below
 * -100 (LUFS or dB), else the value, or with `target` the value in LU
 * relative to it with a sign.
 */
export function levelText(v: number | null | undefined, target?: number): string {
  if (v === null || v === undefined) return fit('--.-', 5);
  // Rounded first: -99.95 would print as "-100.0".
  const r = Math.round(v * 10) / 10;
  if (r <= INF_DB) return fit('-inf', 5);
  if (target === undefined) return fit(r.toFixed(1), 5);
  const d = r - target;
  return fit(`${d >= 0 ? '+' : ''}${d.toFixed(1)}`, 5);
}

/** `text` centred in `width` characters, the extra space at the end when odd. */
export function centre(text: string, width: number): string {
  const pad = Math.max(0, width - text.length);
  return ' '.repeat(Math.floor(pad / 2)) + text + ' '.repeat(Math.ceil(pad / 2));
}
