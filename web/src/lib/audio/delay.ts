// Delay entry and display in milliseconds, video frames or samples. The
// contract stores integer samples at the shape sample rate (DelaySamples).

import type { FrameRate } from '../contract/types';

export type DelayUnit = 'ms' | 'frames' | 'samples';

export const MAX_DELAY_SAMPLES = 240000;

const UNIT_WORDS: Record<string, DelayUnit | 's'> = {
  ms: 'ms',
  s: 's',
  sec: 's',
  f: 'frames',
  fr: 'frames',
  frame: 'frames',
  frames: 'frames',
  smp: 'samples',
  sample: 'samples',
  samples: 'samples',
};

const fps = (fr: FrameRate) => fr.numerator / fr.denominator;

/** Samples as a number in `unit`. */
export function fromSamples(samples: number, unit: DelayUnit, sampleRate: number, frameRate: FrameRate): number {
  if (unit === 'samples') return samples;
  const seconds = samples / sampleRate;
  return unit === 'ms' ? seconds * 1000 : seconds * fps(frameRate);
}

/** Delay text for display, e.g. "40.0 ms", "2.00 fr", "1920 smp". */
export function formatDelay(samples: number, unit: DelayUnit, sampleRate: number, frameRate: FrameRate): string {
  const v = fromSamples(samples, unit, sampleRate, frameRate);
  if (unit === 'samples') return `${Math.round(v)}`;
  return unit === 'ms' ? v.toFixed(1) : v.toFixed(2);
}

/**
 * Typed delay to samples, or null when it does not parse. A bare number is
 * in `unit`; a suffix (ms, s, f, fr, frames, smp, samples) overrides it.
 * The result is rounded to whole samples and clamped to 0..MAX_DELAY_SAMPLES.
 */
export function parseDelay(text: string, unit: DelayUnit, sampleRate: number, frameRate: FrameRate): number | null {
  const m = /^\s*(-?\d+(?:[.,]\d*)?|-?[.,]\d+)\s*([a-z]*)\s*$/i.exec(text);
  if (!m) return null;
  const n = Number(m[1]!.replace(',', '.'));
  const word = m[2]!.toLowerCase();
  const u = word === '' ? unit : UNIT_WORDS[word];
  if (u === undefined || !Number.isFinite(n)) return null;
  const samples =
    u === 'samples' ? n : u === 's' ? n * sampleRate : u === 'ms' ? (n / 1000) * sampleRate : (n / fps(frameRate)) * sampleRate;
  return Math.min(MAX_DELAY_SAMPLES, Math.max(0, Math.round(samples)));
}
