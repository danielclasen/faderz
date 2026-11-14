// Typed values of processing parameters: "2.5k" or "2.5 kHz" is 2500,
// "-3 dB" is -3, "40%" is 0.4, "4:1" is 4, "-inf" or "off" is the
// minimum. Returns null for text that holds no number.

export function parseEntry(text: string, min: number): number | null {
  const t = text.trim().toLowerCase();
  if (t === '-inf' || t === 'off') return min;
  const m = /^([-+]?\d*[.,]?\d+)\s*(k)?/.exec(t);
  if (!m) return null;
  let v = Number(m[1]!.replace(',', '.'));
  if (!Number.isFinite(v)) return null;
  if (m[2]) v *= 1000;
  if (/%\s*$/.test(t)) v /= 100;
  return v;
}
