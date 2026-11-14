// Magnitude response of the strip EQ for display. Uses the RBJ cookbook
// shelf and peak shapes, which match the magnitude of the engine's SVF
// filters for the same frequency, gain and Q.

import type { Eq, EqBand, Hpf } from '../contract/types';

const SR = 48000;

interface Biquad {
  b0: number;
  b1: number;
  b2: number;
  a0: number;
  a1: number;
  a2: number;
}

function bandCoefficients(band: EqBand): Biquad {
  const A = Math.pow(10, band.gain / 40);
  const w0 = (2 * Math.PI * band.freq) / SR;
  const cos = Math.cos(w0);
  const alpha = Math.sin(w0) / (2 * band.q);
  if (band.type === 'peak') {
    return {
      b0: 1 + alpha * A,
      b1: -2 * cos,
      b2: 1 - alpha * A,
      a0: 1 + alpha / A,
      a1: -2 * cos,
      a2: 1 - alpha / A,
    };
  }
  const sq = 2 * Math.sqrt(A) * alpha;
  if (band.type === 'lowShelf') {
    return {
      b0: A * (A + 1 - (A - 1) * cos + sq),
      b1: 2 * A * (A - 1 - (A + 1) * cos),
      b2: A * (A + 1 - (A - 1) * cos - sq),
      a0: A + 1 + (A - 1) * cos + sq,
      a1: -2 * (A - 1 + (A + 1) * cos),
      a2: A + 1 + (A - 1) * cos - sq,
    };
  }
  return {
    b0: A * (A + 1 + (A - 1) * cos + sq),
    b1: -2 * A * (A - 1 + (A + 1) * cos),
    b2: A * (A + 1 + (A - 1) * cos - sq),
    a0: A + 1 - (A - 1) * cos + sq,
    a1: 2 * (A - 1 - (A + 1) * cos),
    a2: A + 1 - (A - 1) * cos - sq,
  };
}

function hpfCoefficients(freq: number): Biquad {
  const w0 = (2 * Math.PI * freq) / SR;
  const cos = Math.cos(w0);
  const alpha = Math.sin(w0) / (2 * Math.SQRT1_2);
  return {
    b0: (1 + cos) / 2,
    b1: -(1 + cos),
    b2: (1 + cos) / 2,
    a0: 1 + alpha,
    a1: -2 * cos,
    a2: 1 - alpha,
  };
}

function magnitudeDb(c: Biquad, freq: number): number {
  const w = (2 * Math.PI * freq) / SR;
  const cos1 = Math.cos(w);
  const cos2 = Math.cos(2 * w);
  const sin1 = Math.sin(w);
  const sin2 = Math.sin(2 * w);
  const nr = c.b0 + c.b1 * cos1 + c.b2 * cos2;
  const ni = -(c.b1 * sin1 + c.b2 * sin2);
  const dr = c.a0 + c.a1 * cos1 + c.a2 * cos2;
  const di = -(c.a1 * sin1 + c.a2 * sin2);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Log-spaced frequencies from 20 Hz to 20 kHz. */
export function logFrequencies(points: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < points; i++) out.push(20 * Math.pow(1000, i / (points - 1)));
  return out;
}

/** Response in dB at each frequency; flat when the EQ and HPF are off. */
export function eqResponse(eq: Eq, hpf: Hpf | null, freqs: readonly number[]): number[] {
  const filters: Biquad[] = [];
  if (eq.enabled) for (const b of eq.bands) if (b.gain !== 0) filters.push(bandCoefficients(b));
  if (hpf?.enabled) filters.push(hpfCoefficients(hpf.freq));
  return freqs.map((f) => filters.reduce((sum, c) => sum + magnitudeDb(c, f), 0));
}
