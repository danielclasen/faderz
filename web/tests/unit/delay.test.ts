import { describe, expect, it } from 'vitest';
import { delayMax, delayRange, formatDelay, MAX_DELAY_SAMPLES, parseDelay } from '../../src/lib/audio/delay';
import type { ChannelShape } from '../../src/lib/contract/types';

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

  // The server refuses a channel delay past the channel's delay line, and any
  // but 0 on a channel without one; buses and auxes take any DelaySamples.
  it('takes a channel up to its delay line, none without one, and a bus or aux up to 5 s', () => {
    const channel = (maxDelay?: number): ChannelShape => ({ id: 'ch1', name: 'Pres 1', width: 1, layout: 'mono', altInput: false, ...(maxDelay === undefined ? {} : { maxDelay }) });
    expect(delayMax(channel(4800))).toBe(4800);
    expect(delayMax(channel())).toBeNull();
    expect(delayMax(undefined)).toBe(MAX_DELAY_SAMPLES);
  });

  it('caps a typed delay at the line it is given', () => {
    expect(parseDelay('1 s', 'ms', SR, PAL, 4800)).toBe(4800);
    expect(parseDelay('3f', 'ms', SR, PAL, 4800)).toBe(4800);
    expect(parseDelay('99.9', 'ms', SR, PAL, 4800)).toBe(4795);
    expect(parseDelay('9 s', 'ms', SR, PAL, MAX_DELAY_SAMPLES)).toBe(MAX_DELAY_SAMPLES);
  });

  it('shows the range in the selected unit', () => {
    expect(delayRange(4800, 'ms', SR, PAL)).toBe('0 to 100.0 ms');
    expect(delayRange(4800, 'frames', SR, PAL)).toBe('0 to 2.50 fr');
    expect(delayRange(4800, 'samples', SR, PAL)).toBe('0 to 4800 smp');
    expect(delayRange(MAX_DELAY_SAMPLES, 'ms', SR, NTSC)).toBe('0 to 5000.0 ms');
  });
});
