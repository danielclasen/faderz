import { describe, expect, it } from 'vitest';
import {
  BAR_FALL_DB_PER_S,
  METER_FLOOR_DB,
  PEAK_HOLD_MS,
  meterMarks,
  meterPosition,
  newLeg,
  stepLeg,
} from '../../src/lib/meters/ballistics';
import { eqResponse, logFrequencies } from '../../src/lib/audio/eq';
import type { Eq } from '../../src/lib/contract/types';
import { decodeMeterFrame, deriveMeterLayout, encodeMeterFrame } from '../../src/lib/contract/meterFrame';
import { flattenMeterFrame } from '../../src/lib/meters/flat';
import { MeterStore } from '../../src/lib/meters/store';
import { exampleShape } from '../../dev/mock/shape';

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

describe('meter marks', () => {
  it('mark full scale, the EBU zone boundaries and the lower decades', () => {
    expect(meterMarks({ alignment: -18, permittedMax: -9 })).toEqual([0, -9, -18, -30, -40, -50, -60]);
  });

  it('follow configured zones, without duplicates', () => {
    expect(meterMarks({ alignment: -20, permittedMax: -10 })).toEqual([0, -10, -20, -30, -40, -50, -60]);
    expect(meterMarks({ alignment: -30, permittedMax: 0 })).toEqual([0, -30, -40, -50, -60]);
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

describe('flattened meter frames', () => {
  it('reads back every element of a decoded frame from the transferred values', () => {
    const shape = exampleShape();
    const layout = deriveMeterLayout(shape);
    let x = 0;
    const legs = (n: number) => Array.from({ length: n }, () => -((x++ % 100) + 0.5));
    const bus = (n: number, i: number) => ({
      peak: legs(n),
      truePeak: legs(n),
      rms: legs(n),
      loudness: {
        momentary: -23 - i,
        shortTerm: i % 2 ? null : -20,
        integrated: null,
        lra: 4.5,
        maxMomentary: -20 - i,
        maxShortTerm: null,
        integratedValid: i % 2 === 0,
        lraStable: i % 2 === 1,
      },
      correlation: i % 2 ? null : 0.5 + i / 10,
    });
    const values = {
      channels: layout.channels.map((e, i) => ({ peak: legs(e.legs), truePeak: legs(e.legs), momentary: i % 3 ? -30 - i : null })),
      buses: layout.buses.map((e, i) => bus(e.legs, i)),
      auxes: layout.auxes.map((e, i) => bus(e.legs, i + 1)),
      mixMinus: layout.mixMinus.map((e, i) => bus(e.legs, i)),
    };
    const frame = decodeMeterFrame(layout, encodeMeterFrame(layout, { sequence: 9, replica: 'r3' }, values));
    const store = new MeterStore();
    store.setShape(shape);
    store.ingest(frame.replica, flattenMeterFrame(layout, frame), 1);
    expect(store.replica).toBe('r3');
    layout.channels.forEach((e, i) => {
      const r = store.get('channels', e.id)!;
      expect(r.peak).toEqual(frame.channels[i]!.peak.map((v) => Math.fround(v)));
      expect(r.truePeak).toEqual(frame.channels[i]!.truePeak.map((v) => Math.fround(v)));
      const m = frame.channels[i]!.momentary;
      expect(r.momentary).toBe(m === null ? null : Math.fround(m));
    });
    for (const group of ['buses', 'auxes', 'mixMinus'] as const) {
      layout[group].forEach((e, i) => {
        const r = store.get(group, e.id)!;
        const want = frame[group][i]!;
        expect(r.truePeak).toEqual(want.truePeak.map((v) => Math.fround(v)));
        expect(r.loudness).toEqual({
          momentary: want.loudness.momentary === null ? null : Math.fround(want.loudness.momentary),
          shortTerm: want.loudness.shortTerm === null ? null : Math.fround(want.loudness.shortTerm),
          integrated: null,
          lra: Math.fround(4.5),
          maxMomentary: Math.fround(want.loudness.maxMomentary!),
          maxShortTerm: null,
          integratedValid: want.loudness.integratedValid,
          lraStable: want.loudness.lraStable,
        });
        expect(r.correlation).toBe(want.correlation === null ? null : Math.fround(want.correlation));
      });
    }
    store.clear();
    expect(store.get('channels', 'ch1')).toBeUndefined();
  });
});
