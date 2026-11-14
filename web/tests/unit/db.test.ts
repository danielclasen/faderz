import { describe, expect, it } from 'vitest';
import {
  FADER_OFF_DB,
  FADER_TAPER,
  dbToGain,
  dbToPosition,
  dragDb,
  effectiveGainDb,
  formatDb,
  formatPan,
  positionToDb,
  keyStepDb,
  stepDb,
} from '../../src/lib/audio/db';

describe('fader taper', () => {
  it('puts 0 dB at three quarters of the travel and spans off to +10 dB', () => {
    expect(dbToPosition(0)).toBe(0.75);
    expect(dbToPosition(10)).toBe(1);
    expect(dbToPosition(FADER_OFF_DB)).toBe(0);
  });

  it('is strictly monotonic', () => {
    for (let i = 1; i < FADER_TAPER.length; i++) {
      expect(FADER_TAPER[i]![0]).toBeGreaterThan(FADER_TAPER[i - 1]![0]);
      expect(FADER_TAPER[i]![1]).toBeGreaterThan(FADER_TAPER[i - 1]![1]);
    }
  });

  it('round-trips every 0.1 dB step from -60 to +10 dB', () => {
    for (let t = -600; t <= 100; t++) {
      const db = t / 10;
      expect(positionToDb(dbToPosition(db))).toBeCloseTo(db, 6);
    }
  });

  it('snaps the bottom of the travel to off and clamps beyond the ends', () => {
    expect(positionToDb(0)).toBe(FADER_OFF_DB);
    expect(positionToDb(-1)).toBe(FADER_OFF_DB);
    expect(positionToDb(2)).toBe(10);
    expect(dbToPosition(30)).toBe(1);
  });
});

describe('drag', () => {
  it('moves the fader by the dragged fraction of the track; up is louder', () => {
    expect(dragDb(0, -50, 400, false)).toBe(5);
    expect(dragDb(0, 50, 400, false)).toBe(-5);
  });

  it('moves ten times less in fine mode', () => {
    expect(dragDb(0, -100, 400, true)).toBe(dragDb(0, -10, 400, false));
    expect(dragDb(0, -100, 400, true)).toBe(1);
  });
});

describe('keyboard steps', () => {
  it('steps in dB, leaves off at -60 dB and falls back to off below it', () => {
    expect(stepDb(0, 1)).toBe(1);
    expect(stepDb(0, -0.1)).toBe(-0.1);
    expect(stepDb(FADER_OFF_DB, 1)).toBe(-60);
    expect(stepDb(-60, -1)).toBe(FADER_OFF_DB);
    expect(stepDb(10, 1)).toBe(10);
    expect([keyStepDb(0), keyStepDb(-19.5), keyStepDb(-30), keyStepDb(-50)]).toEqual([0.5, 0.5, 1, 3]);
  });
});

describe('gain', () => {
  it('converts dB to linear gain with off as silence', () => {
    expect(dbToGain(0)).toBe(1);
    expect(dbToGain(-6)).toBeCloseTo(0.501, 3);
    expect(dbToGain(FADER_OFF_DB)).toBe(0);
  });

  it('sums VCA offsets onto the fader and clamps at +20 dB', () => {
    expect(effectiveGainDb(-10, [3, -2])).toBe(-9);
    expect(effectiveGainDb(10, [10, 10])).toBe(20);
    expect(effectiveGainDb(FADER_OFF_DB, [10])).toBe(FADER_OFF_DB);
    expect(effectiveGainDb(0, [FADER_OFF_DB])).toBe(FADER_OFF_DB);
  });

  it('formats dB and pan for strips', () => {
    expect(formatDb(FADER_OFF_DB)).toBe('-inf');
    expect(formatDb(0)).toBe('0.0');
    expect(formatDb(-0.04)).toBe('0.0');
    expect(formatDb(3.5)).toBe('+3.5');
    expect(formatPan(0)).toBe('C');
    expect(formatPan(-0.42)).toBe('L42');
    expect(formatPan(1)).toBe('R100');
  });
});
