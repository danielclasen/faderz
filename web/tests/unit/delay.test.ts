import { describe, expect, it } from 'vitest';
import { formatDelay, MAX_DELAY_SAMPLES, parseDelay } from '../../src/lib/audio/delay';

const SR = 48000;
const PAL = { numerator: 25, denominator: 1 };
const NTSC = { numerator: 30000, denominator: 1001 };

describe('delay entry', () => {
  it('reads a bare number in the selected unit', () => {
    expect(parseDelay('40', 'ms', SR, PAL)).toBe(1920);
    expect(parseDelay('2', 'frames', SR, PAL)).toBe(3840);
    expect(parseDelay('1921', 'samples', SR, PAL)).toBe(1921);
  });

  it('takes a unit suffix over the selected unit', () => {
    expect(parseDelay('2f', 'ms', SR, PAL)).toBe(3840);
    expect(parseDelay('1.5 s', 'frames', SR, PAL)).toBe(72000);
    expect(parseDelay('40,5ms', 'samples', SR, PAL)).toBe(1944);
    expect(parseDelay('1 frame', 'ms', SR, NTSC)).toBe(1602);
  });

  it('refuses text that is no delay and clamps to 0 to 5 s', () => {
    expect(parseDelay('soon', 'ms', SR, PAL)).toBeNull();
    expect(parseDelay('3 parsecs', 'ms', SR, PAL)).toBeNull();
    expect(parseDelay('-5', 'ms', SR, PAL)).toBe(0);
    expect(parseDelay('9 s', 'ms', SR, PAL)).toBe(MAX_DELAY_SAMPLES);
    expect(MAX_DELAY_SAMPLES / SR).toBe(5);
  });

  it('shows ms to 0.1, frames to 0.01 and whole samples', () => {
    expect(formatDelay(1920, 'ms', SR, PAL)).toBe('40.0');
    expect(formatDelay(1920, 'frames', SR, PAL)).toBe('1.00');
    expect(formatDelay(1602, 'frames', SR, NTSC)).toBe('1.00');
    expect(formatDelay(1920, 'samples', SR, PAL)).toBe('1920');
  });
});
