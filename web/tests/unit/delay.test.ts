import { describe, expect, it } from 'vitest';
import { delayMax, delayRange, formatDelay, parseDelay } from '../../src/lib/audio/delay';
import type { BusShape, ChannelShape, ElementShape } from '../../src/lib/contract/types';

const SR = 48000;
const PAL = { numerator: 25, denominator: 1 };
const NTSC = { numerator: 30000, denominator: 1001 };
// The longest line the engine allocates, 5 s at 48 kHz.
const MAX_DELAY_SAMPLES = 240000;

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

  // The entry refuses a value outside the range; capping it would write a
  // delay nobody typed.
  it('refuses text that is no delay and never clamps one out of range', () => {
    expect(parseDelay('soon', 'ms', SR, PAL)).toBeNull();
    expect(parseDelay('3 parsecs', 'ms', SR, PAL)).toBeNull();
    expect(parseDelay('-5', 'ms', SR, PAL)).toBe(-240);
    expect(parseDelay('9 s', 'ms', SR, PAL)).toBe(432000);
  });

  it('shows ms to 0.1, frames to 0.01 and whole samples', () => {
    expect(formatDelay(1920, 'ms', SR, PAL)).toBe('40.0');
    expect(formatDelay(1920, 'frames', SR, PAL)).toBe('1.00');
    expect(formatDelay(1602, 'frames', SR, NTSC)).toBe('1.00');
    expect(formatDelay(1920, 'samples', SR, PAL)).toBe('1920');
  });

  // The server refuses a delay past the delay line of a channel, bus or
  // aux, and any but 0 on one without a line.
  it('takes a channel, bus or aux up to its delay line, and none without one', () => {
    const channel = (maxDelay?: number): ChannelShape => ({ id: 'ch1', name: 'Pres 1', width: 1, layout: 'mono', altInput: false, ...(maxDelay === undefined ? {} : { maxDelay }) });
    const bus = (maxDelay?: number): BusShape => ({ id: 'pgm', name: 'PGM', kind: 'program', width: 2, layout: 'stereo', ...(maxDelay === undefined ? {} : { maxDelay }) });
    expect(delayMax(channel(4800))).toBe(4800);
    expect(delayMax(channel())).toBeNull();
    expect(delayMax(bus(MAX_DELAY_SAMPLES))).toBe(MAX_DELAY_SAMPLES);
    expect(delayMax(bus())).toBeNull();
    const aux: ElementShape = { id: 'aux1', name: 'IFB 1', width: 1, layout: 'mono', maxDelay: 480 };
    expect(delayMax(aux)).toBe(480);
  });

  it('shows the range in the selected unit', () => {
    expect(delayRange(4800, 'ms', SR, PAL)).toBe('0 to 100.0 ms');
    expect(delayRange(4800, 'frames', SR, PAL)).toBe('0 to 2.50 fr');
    expect(delayRange(4800, 'samples', SR, PAL)).toBe('0 to 4800 smp');
    expect(delayRange(MAX_DELAY_SAMPLES, 'ms', SR, NTSC)).toBe('0 to 5000.0 ms');
  });
});
