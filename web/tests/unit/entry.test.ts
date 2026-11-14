import { describe, expect, it } from 'vitest';
import { parseEntry } from '../../src/lib/audio/entry';

describe('typed parameter values', () => {
  it('reads plain numbers, units, kilo, percent and ratios', () => {
    expect(parseEntry('-3', -90)).toBe(-3);
    expect(parseEntry(' -3.5 dB', -90)).toBe(-3.5);
    expect(parseEntry('2.5k', 20)).toBe(2500);
    expect(parseEntry('2,5 kHz', 20)).toBe(2500);
    expect(parseEntry('120 Hz', 20)).toBe(120);
    expect(parseEntry('40%', 0)).toBeCloseTo(0.4);
    expect(parseEntry('4.0:1', 1)).toBe(4);
  });

  it('reads off and -inf as the minimum and refuses text without a number', () => {
    expect(parseEntry('-inf', -90)).toBe(-90);
    expect(parseEntry('OFF', -90)).toBe(-90);
    expect(parseEntry('loud', -90)).toBeNull();
  });
});
