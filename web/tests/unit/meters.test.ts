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

  it('returns 20 dB in 1.7 s, as an IEC 60268-18 digital peak meter', () => {
    const leg = newLeg();
    stepLeg(leg, -10, 0, 40);
    for (let t = 40; t <= 1700; t += 40) stepLeg(leg, METER_FLOOR_DB, t, 40);
    // 1.7 s = 42.5 frames; the last step lands at 1.68 s.
    expect(leg.bar).toBeCloseTo(-10 - (20 * 1680) / 1700, 6);
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
      monitor: layout.monitor.map((e) => ({ peak: legs(e.legs) })),
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

describe('meter store: true peak between display ticks', () => {
  const shape = exampleShape();
  const layout = deriveMeterLayout(shape);
  /** Offset of the true-peak legs of bus `id` in a flattened frame (meters/flat.ts). */
  const tpOffset = (id: string) => {
    let offset = layout.channels.reduce((a, e) => a + 2 * e.legs + 1, 0);
    for (const e of layout.buses) {
      if (e.id === id) return offset + e.legs;
      offset += 3 * e.legs + 8;
    }
    throw new Error(id);
  };
  /** A frame with every value at -40 dB but the first true-peak leg of pgm20 at `tp`. */
  const frame = (tp: number) => {
    const flat = new Float32Array(layout.valueCount).fill(-40);
    flat[tpOffset('pgm20')] = tp;
    return flat;
  };

  it('keeps the highest true peak of every frame since a given time, not only the latest', () => {
    const store = new MeterStore();
    store.setShape(shape);
    store.ingest('r1', frame(-12), 40);
    store.ingest('r1', frame(-3.5), 80);
    store.ingest('r1', frame(-20), 120);
    expect(store.get('buses', 'pgm20')!.truePeak[0]).toBe(-20);
    const out: number[] = [];
    expect(store.maxSince('buses', 'pgm20', 'truePeak', 0, out)).toBe(true);
    expect(out[0]).toBeCloseTo(-3.5, 5);
    expect(out[1]).toBe(-40);
    expect(store.maxSince('buses', 'pgm20', 'truePeak', 80, out)).toBe(true);
    expect(out[0]).toBe(-20);
    expect(store.maxSince('buses', 'pgm20', 'truePeak', 120, out)).toBe(false);
  });

  it('latches a true-peak over from any one frame until it is cleared', () => {
    const store = new MeterStore();
    store.setShape(shape);
    store.truePeakMax = -1;
    store.ingest('r1', frame(-1), 40);
    expect(store.overs.has('buses/pgm20')).toBe(false);
    store.ingest('r1', frame(-0.5), 80);
    store.ingest('r1', frame(-30), 120);
    store.clear();
    expect(store.overs.has('buses/pgm20')).toBe(true);
    expect([...store.overs]).toEqual(['buses/pgm20']);
    store.clearOvers('buses', ['pgm20']);
    expect(store.overs.size).toBe(0);
  });

  it('latches the true peak of a channel direct out', () => {
    const store = new MeterStore();
    store.setShape(shape);
    const flat = new Float32Array(layout.valueCount).fill(-40);
    // ch1 is the first channel, one leg: peak, then true peak.
    flat[1] = 0.2;
    store.ingest('r1', flat, 40);
    expect([...store.overs]).toEqual(['channels/ch1']);
  });

  it('shows a channel true peak only where the channel feeds a direct out', () => {
    const store = new MeterStore();
    store.setShape(shape);
    const direct = new Set(shape.outputs.map((o) => o.source));
    const without = layout.channels.find((c) => !direct.has(c.id))!;
    expect(direct.has('ch1')).toBe(true);
    // Every channel's true-peak leg above the limit.
    const flat = new Float32Array(layout.valueCount).fill(-40);
    let offset = 0;
    for (const c of layout.channels) {
      for (let i = 0; i < c.legs; i++) flat[offset + c.legs + i] = 0.5;
      offset += 2 * c.legs + 1;
    }
    store.ingest('r1', flat, 40);
    const channelOvers = [...store.overs].filter((k) => k.startsWith('channels/')).sort();
    expect(channelOvers).toEqual([...direct].filter((id) => layout.channels.some((c) => c.id === id)).map((id) => `channels/${id}`).sort());
    expect(store.hasTruePeak('channels', 'ch1')).toBe(true);
    expect(store.hasTruePeak('channels', without.id)).toBe(false);
    expect(store.hasTruePeak('buses', 'pgm20')).toBe(true);
    expect(store.hasTruePeak('monitor', 'mon')).toBe(false);
    const out: number[] = [];
    expect(store.maxSince('channels', without.id, 'truePeak', 0, out)).toBe(false);
    expect(store.maxSince('channels', 'ch1', 'truePeak', 0, out)).toBe(true);
    // A shape without the direct out drops the channel's over.
    store.setShape({ ...shape, outputs: shape.outputs.filter((o) => o.source !== 'ch1') });
    expect(store.overs.has('channels/ch1')).toBe(false);
    expect(store.hasTruePeak('channels', 'ch1')).toBe(false);
  });

  it('keeps latched overs across a snapshot of the same shape and clears them when the layout changes', () => {
    const store = new MeterStore();
    store.setShape(shape);
    store.ingest('r1', frame(0), 40);
    expect(store.overs.has('buses/pgm20')).toBe(true);
    // A reconnect without resume, a new epoch or a rewind sends a snapshot.
    store.setShape(exampleShape());
    expect(store.overs.has('buses/pgm20')).toBe(true);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBe(0);
    store.setShape(exampleShape(shape.channels.length + 1));
    expect(store.overs.size).toBe(0);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBeNull();
  });

  it('holds the highest true peak since the last reset, not the latest frames', () => {
    const store = new MeterStore();
    store.setShape(shape);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBeNull();
    store.ingest('r1', frame(-12), 40);
    store.ingest('r1', frame(-3.5), 80);
    for (let t = 120; t < 120 + 40 * 50; t += 40) store.ingest('r1', frame(-20), t);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBeCloseTo(-3.5, 5);
    store.clearOvers('buses', ['pgm20']);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBeNull();
    store.ingest('r1', frame(-20), 4000);
    expect(store.truePeakSinceReset('buses', 'pgm20')).toBe(-20);
  });

  describe('a loudness reset made on any client', () => {
    /** Offset of the loudness values of bus `id` (meters/flat.ts). */
    const loudOffset = (id: string) => tpOffset(id) + 2 * layout.buses.find((b) => b.id === id)!.legs;
    const loud = (tp: number, l: { maxMomentary: number; maxShortTerm: number; lraStable: boolean }) => {
      const flat = frame(tp);
      const t = loudOffset('pgm20');
      flat[t + 4] = l.maxMomentary;
      flat[t + 5] = l.maxShortTerm;
      flat[t + 7] = 1 | (l.lraStable ? 2 : 0);
      return flat;
    };

    it('clears the over and the true-peak maximum when the maxima since reset fall', () => {
      const store = new MeterStore();
      store.setShape(shape);
      store.ingest('r1', loud(0, { maxMomentary: -20, maxShortTerm: -22, lraStable: true }), 40);
      store.ingest('r1', loud(-30, { maxMomentary: -18, maxShortTerm: -21, lraStable: true }), 80);
      expect(store.overs.has('buses/pgm20')).toBe(true);
      // Another client's RESET: the maxima start again from the current level.
      store.ingest('r1', loud(-30, { maxMomentary: -25, maxShortTerm: Number.NaN, lraStable: false }), 120);
      expect(store.overs.has('buses/pgm20')).toBe(false);
      expect(store.truePeakSinceReset('buses', 'pgm20')).toBe(-30);
    });

    it('is seen from integrated loudness turning null when the maxima do not move', () => {
      const store = new MeterStore();
      store.setShape(shape);
      const t = loudOffset('pgm20');
      const a = loud(0, { maxMomentary: -20, maxShortTerm: -22, lraStable: false });
      a[t + 2] = -23;
      a[t + 3] = 4;
      store.ingest('r1', a, 40);
      // Reset while the momentary level equals its maximum.
      const b = loud(-30, { maxMomentary: -20, maxShortTerm: -22, lraStable: false });
      b[t + 2] = Number.NaN;
      b[t + 3] = 4;
      store.ingest('r1', b, 80);
      expect(store.overs.has('buses/pgm20')).toBe(false);
    });

    it('is seen from LRA alone turning null', () => {
      const store = new MeterStore();
      store.setShape(shape);
      const t = loudOffset('pgm20');
      const a = loud(0, { maxMomentary: -20, maxShortTerm: -22, lraStable: false });
      a[t + 2] = -23;
      a[t + 3] = 4;
      store.ingest('r1', a, 40);
      // Integrated already measured again, LRA not yet: only LRA shows it.
      const b = loud(-30, { maxMomentary: -20, maxShortTerm: -22, lraStable: false });
      b[t + 2] = -23;
      b[t + 3] = Number.NaN;
      store.ingest('r1', b, 80);
      expect(store.overs.has('buses/pgm20')).toBe(false);
    });

    it('is never read on a channel, whose values past its legs belong to the next channels', () => {
      const store = new MeterStore();
      store.setShape(shape);
      // ch1 is the first channel, one leg, a direct out: peak, true peak, momentary.
      const a = new Float32Array(layout.valueCount).fill(-40);
      a[1] = 0.2;
      store.ingest('r1', a, 40);
      expect(store.overs.has('channels/ch1')).toBe(true);
      // Where a bus keeps its maxima and integrated, ch1 is followed by
      // other channels' values; here they fall to null.
      const b = new Float32Array(layout.valueCount).fill(-40);
      for (let i = 3; i < 3 + 8; i++) b[i] = Number.NaN;
      store.ingest('r1', b, 80);
      expect(store.overs.has('channels/ch1')).toBe(true);
      expect(store.truePeakSinceReset('channels', 'ch1')).toBeCloseTo(0.2, 5);
    });

    it('is seen from LRA turning unstable when the maxima do not move', () => {
      const store = new MeterStore();
      store.setShape(shape);
      store.ingest('r1', loud(0, { maxMomentary: -20, maxShortTerm: -22, lraStable: true }), 40);
      store.ingest('r1', loud(-30, { maxMomentary: -20, maxShortTerm: -22, lraStable: false }), 80);
      expect(store.overs.has('buses/pgm20')).toBe(false);
    });

    it('is not read from the other measurement of a new replica', () => {
      const store = new MeterStore();
      store.setShape(shape);
      store.ingest('r1', loud(0, { maxMomentary: -20, maxShortTerm: -22, lraStable: true }), 40);
      store.ingest('r2', loud(-30, { maxMomentary: -21, maxShortTerm: -23, lraStable: true }), 80);
      expect(store.overs.has('buses/pgm20')).toBe(true);
      store.ingest('r2', loud(-30, { maxMomentary: -21, maxShortTerm: -23, lraStable: true }), 120);
      expect(store.overs.has('buses/pgm20')).toBe(true);
    });
  });
});
