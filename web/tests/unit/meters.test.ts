import { describe, expect, it } from 'vitest';
import {
  BAR_FALL_DB_PER_S,
  METER_FLOOR_DB,
  PEAK_HOLD_MS,
  meterPosition,
  newLeg,
  stepLeg,
} from '../../src/lib/meters/ballistics';
import { eqResponse, logFrequencies } from '../../src/lib/audio/eq';
import type { Eq } from '../../src/lib/contract/types';

describe('meter ballistics', () => {
  it('rises instantly and falls at the bar fall rate', () => {
    const leg = newLeg();
    stepLeg(leg, -10, 0, 40);
    expect(leg.bar).toBe(-10);
    stepLeg(leg, -60, 1000, 1000);
    expect(leg.bar).toBeCloseTo(-10 - BAR_FALL_DB_PER_S, 6);
  });

  it('holds the peak for the hold time, then lets it fall', () => {
    const leg = newLeg();
    stepLeg(leg, -3, 0, 40);
    stepLeg(leg, -40, PEAK_HOLD_MS - 10, 40);
    expect(leg.hold).toBe(-3);
    stepLeg(leg, -40, PEAK_HOLD_MS + 500, 500);
    expect(leg.hold).toBeLessThan(-3);
    expect(leg.hold).toBeGreaterThanOrEqual(leg.bar);
  });

  it('never goes below the floor', () => {
    const leg = newLeg();
    stepLeg(leg, -120, 0, 5000);
    expect(leg.bar).toBe(METER_FLOOR_DB);
  });

  it('maps the scale monotonically from floor to 0 dBFS', () => {
    expect(meterPosition(METER_FLOOR_DB)).toBe(0);
    expect(meterPosition(0)).toBe(1);
    expect(meterPosition(5)).toBe(1);
    let last = -1;
    for (let db = -60; db <= 0; db += 0.5) {
      const p = meterPosition(db);
      expect(p).toBeGreaterThan(last);
      last = p;
    }
  });
});

describe('EQ display response', () => {
  const eq = (gain: number, enabled = true): Eq => ({
    enabled,
    bands: [
      { type: 'lowShelf', freq: 100, gain: 0, q: 0.7 },
      { type: 'peak', freq: 1000, gain, q: 1 },
      { type: 'peak', freq: 2500, gain: 0, q: 1 },
      { type: 'highShelf', freq: 8000, gain: 0, q: 0.7 },
    ],
  });

  it('peaks at the band gain at the band frequency', () => {
    const [atBand] = eqResponse(eq(6), null, [1000]);
    expect(atBand).toBeCloseTo(6, 1);
  });

  it('is flat with the EQ and HPF out', () => {
    const r = eqResponse(eq(6, false), { enabled: false, freq: 80 }, logFrequencies(16));
    expect(r.every((v) => v === 0)).toBe(true);
  });

  it('attenuates below the HPF corner', () => {
    const [low, high] = eqResponse(eq(0), { enabled: true, freq: 100 }, [30, 5000]);
    expect(low).toBeLessThan(-15);
    expect(Math.abs(high!)).toBeLessThan(0.1);
  });
});
