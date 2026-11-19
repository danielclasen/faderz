import { describe, expect, it } from 'vitest';
import { applyImage, legMatrix, ReplicaMeters, SourceModel } from '../../dev/mock/signals';
import { exampleShape, initialState } from '../../dev/mock/shape';
import { LAYOUT_LABELS } from '../../src/lib/contract/types';

const leg = (label: string) => LAYOUT_LABELS['5.1'].indexOf(label);

describe('mock surround panner', () => {
  it('puts a centred mono source in the centre leg only without divergence', () => {
    const m = legMatrix('mono', '5.1', 0, -1);
    expect(m[leg('C')]![0]).toBeCloseTo(1);
    expect(m[leg('L')]![0]).toBeCloseTo(0);
    expect(m[leg('R')]![0]).toBeCloseTo(0);
  });

  it('spreads it to a phantom centre at full divergence, keeping its power', () => {
    const m = legMatrix('mono', '5.1', 0, -1, { divergence: 1 });
    expect(m[leg('C')]![0]).toBeCloseTo(0);
    expect(m[leg('L')]![0]! + m[leg('R')]![0]!).toBeCloseTo(1);
  });

  it('feeds the LFE leg only when the LFE send is open', () => {
    expect(legMatrix('mono', '5.1', 0, -1)[leg('LFE')]![0]).toBe(0);
    expect(legMatrix('mono', '5.1', 0, -1, { lfe: -6 })[leg('LFE')]![0]).toBeCloseTo(10 ** (-6 / 10));
  });
});

describe('mock surround panner law', () => {
  const power = (m: number[][]) => m.reduce((sum, row, d) => sum + (LAYOUT_LABELS['5.1'][d] === 'LFE' ? 0 : row[0]!), 0);

  it('keeps constant power over the field, on 5.1 and 7.1', () => {
    for (const dst of ['5.1', '7.1'] as const) {
      for (let lr = -1; lr <= 1; lr += 0.25) {
        for (let fr = -1; fr <= 1; fr += 0.25) {
          for (const divergence of [0, 0.4, 1]) {
            const m = legMatrix('mono', dst, lr, fr, { divergence });
            const total = m.reduce((sum, row, d) => sum + (LAYOUT_LABELS[dst][d] === 'LFE' ? 0 : row[0]!), 0);
            expect(total).toBeCloseTo(1);
          }
        }
      }
    }
    expect(power(legMatrix('mono', '5.1', 0.3, 0.2))).toBeCloseTo(1);
  });

  it('pans pairwise at the front and between the surrounds at the rear', () => {
    const half = legMatrix('mono', '5.1', -0.5, -1);
    expect(half[leg('L')]![0]).toBeCloseTo(0.5);
    expect(half[leg('C')]![0]).toBeCloseTo(0.5);
    const rear = legMatrix('mono', '5.1', 0, 1);
    expect(rear[leg('Ls')]![0]).toBeCloseTo(0.5);
    expect(rear[leg('Rs')]![0]).toBeCloseTo(0.5);
    expect(rear[leg('C')]![0]).toBeCloseTo(0);
    const middle = legMatrix('mono', '5.1', 0, 0);
    expect(middle[leg('C')]![0]).toBeCloseTo(0.5);
    expect(middle[leg('Ls')]![0]).toBeCloseTo(0.25);
  });

  it('balances a stereo source and applies its image first', () => {
    const balanced = legMatrix('stereo', 'stereo', -0.5, -1);
    expect(balanced[0]![0]).toBeCloseTo(1);
    expect(balanced[1]![1]).toBeCloseTo(0.25);
    const left = legMatrix('stereo', 'stereo', 0, -1, {}, 'leftToBoth');
    expect(left[0]![0]).toBeCloseTo(1);
    expect(left[1]![0]).toBeCloseTo(1);
    expect(left[1]![1]).toBeCloseTo(0);
    const swapped = legMatrix('stereo', 'stereo', 0, -1, {}, 'swap');
    expect(swapped[0]![1]).toBeCloseTo(1);
    expect(swapped[0]![0]).toBeCloseTo(0);
  });
});

describe('mock stereo image', () => {
  it('feeds one leg to both or swaps the legs', () => {
    expect(applyImage('normal', [-10, -20])).toEqual([-10, -20]);
    expect(applyImage('leftToBoth', [-10, -20])).toEqual([-10, -10]);
    expect(applyImage('rightToBoth', [-10, -20])).toEqual([-20, -20]);
    expect(applyImage('swap', [-10, -20])).toEqual([-20, -10]);
  });
});

describe('mock bus limiter', () => {
  it('holds the true peak of a hot programme bus at its ceiling', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const sources = new SourceModel(shape);
    for (const c of shape.channels) sources.levels.set(c.id, Array<number>(c.width).fill(-6));
    for (const c of Object.values(state.channels)) c.fader = 10;
    const pgm = shape.buses.findIndex((b) => b.id === 'pgm20');
    const meters = new ReplicaMeters(shape);
    state.buses['pgm20']!.limiter = { enabled: false, ceiling: -1 };
    expect(Math.max(...meters.frame(state, sources).buses[pgm]!.truePeak)).toBeGreaterThan(-1);
    state.buses['pgm20']!.limiter = { enabled: true, ceiling: -2 };
    expect(Math.max(...meters.frame(state, sources).buses[pgm]!.truePeak)).toBeLessThanOrEqual(-2);
  });
});

describe('mock loudness measurement', () => {
  it('holds integrated loudness, LRA and the maxima while paused and goes on measuring momentary and short-term', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const sources = new SourceModel(shape);
    const meters = new ReplicaMeters(shape);
    const pgm = shape.buses.findIndex((b) => b.id === 'pgm20');
    const level = (db: number) => {
      for (const c of shape.channels) sources.levels.set(c.id, Array<number>(c.width).fill(db));
    };
    const run = (n: number) => {
      let last = meters.frame(state, sources).buses[pgm]!.loudness;
      for (let i = 1; i < n; i++) last = meters.frame(state, sources).buses[pgm]!.loudness;
      return last;
    };
    level(-40);
    const running = run(200);
    expect(running.integrated).not.toBeNull();

    state.loudness['pgm20']!.running = false;
    level(-10);
    const paused = run(200);
    expect(paused.momentary!).toBeGreaterThan(running.momentary! + 20);
    expect(paused.shortTerm!).toBeGreaterThan(running.shortTerm! + 20);
    expect(paused.integrated).toBe(running.integrated);
    expect(paused.lra).toBe(running.lra);
    expect(paused.maxMomentary).toBe(running.maxMomentary);
    expect(paused.maxShortTerm).toBe(running.maxShortTerm);

    state.loudness['pgm20']!.running = true;
    const resumed = run(200);
    expect(resumed.integrated!).toBeGreaterThan(running.integrated! + 5);
    expect(resumed.maxMomentary!).toBeGreaterThan(running.maxMomentary! + 20);
  });

  it('lets no paused audio into the integration: a pause drops the windows in progress and running starts new ones', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const sources = new SourceModel(shape);
    const meters = new ReplicaMeters(shape);
    const pgm = shape.buses.findIndex((b) => b.id === 'pgm20');
    const level = (db: number) => {
      for (const c of shape.channels) sources.levels.set(c.id, Array<number>(c.width).fill(db));
    };
    const run = (n: number) => {
      let last = meters.frame(state, sources).buses[pgm]!.loudness;
      for (let i = 1; i < n; i++) last = meters.frame(state, sources).buses[pgm]!.loudness;
      return last;
    };
    level(-40);
    const quiet = run(300);
    expect(quiet.lra).not.toBeNull();
    expect(quiet.maxShortTerm).not.toBeNull();
    // A loud break while paused, then quiet programme again.
    state.loudness['pgm20']!.running = false;
    level(-10);
    run(200);
    level(-40);
    state.loudness['pgm20']!.running = true;
    const after = run(300);
    expect(after.maxShortTerm!).toBeLessThan(quiet.maxShortTerm! + 1);
    expect(after.maxMomentary!).toBeLessThan(quiet.maxMomentary! + 1);
    expect(Math.abs(after.integrated! - quiet.integrated!)).toBeLessThan(1);
    expect(after.lra!).toBeLessThan(quiet.lra! + 1);
  });

  it('counts only running time towards a stable LRA', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const sources = new SourceModel(shape);
    const meters = new ReplicaMeters(shape);
    const pgm = shape.buses.findIndex((b) => b.id === 'pgm20');
    const frames = (n: number) => {
      let stable = false;
      for (let i = 0; i < n; i++) stable = meters.frame(state, sources).buses[pgm]!.loudness.lraStable;
      return stable;
    };
    // 60 s of meter periods at 40 ms is 1500; half of them run.
    expect(frames(750)).toBe(false);
    state.loudness['pgm20']!.running = false;
    expect(frames(1500)).toBe(false);
    state.loudness['pgm20']!.running = true;
    expect(frames(749)).toBe(false);
    expect(frames(2)).toBe(true);
  });
});

describe('mock line-up tone', () => {
  it('replaces the mix of its target outputs at the selected level and leaves the others', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const meters = new ReplicaMeters(shape);
    const sources = new SourceModel(shape);
    state.tone = { on: true, level: -20, ident: false, targets: ['pgm20'] };
    const frame = meters.frame(state, sources);
    const pgm = shape.buses.findIndex((b) => b.id === 'pgm20');
    const me = shape.buses.findIndex((b) => b.id === 'me');
    expect(frame.buses[pgm]!.peak).toEqual([-20, -20]);
    expect(frame.buses[pgm]!.rms).toEqual([-23, -23]);
    expect(frame.buses[me]!.peak).not.toEqual([-20, -20]);
  });
});

describe('mock monitor meter', () => {
  const setup = () => {
    const shape = exampleShape();
    const state = initialState(shape);
    const sources = new SourceModel(shape);
    for (const c of shape.channels) sources.levels.set(c.id, Array<number>(c.width).fill(-120));
    sources.levels.set('ch1', [-10]);
    sources.levels.set('ch2', [-40]);
    for (const c of Object.values(state.channels)) {
      c.fader = -90;
      c.trim = 0;
      c.legTrim = c.legTrim.map(() => 0);
      c.comp.enabled = false;
    }
    return { shape, state, sources, meters: new ReplicaMeters(shape) };
  };
  const top = (peak: number[]) => Math.max(...peak);

  it('meters the source while nobody listens and the listen sum while somebody does', () => {
    const { state, sources, meters } = setup();
    expect(top(meters.frame(state, sources).monitor[0]!.peak)).toBeLessThan(-100);
    // PFL listens before the fader: ch1 is heard though its fader is closed.
    state.channels['ch1']!.solo = 'pfl';
    expect(top(meters.frame(state, sources).monitor[0]!.peak)).toBeGreaterThan(-20);
  });

  it('gives PFL priority over AFL', () => {
    const { state, sources, meters } = setup();
    state.channels['ch2']!.solo = 'pfl';
    state.channels['ch1']!.solo = 'afl';
    state.channels['ch1']!.fader = 0;
    // Only ch2 (-40 dB) is summed while its PFL is on, not ch1 at -10 dB.
    expect(top(meters.frame(state, sources).monitor[0]!.peak)).toBeLessThan(-20);
    state.channels['ch2']!.solo = 'off';
    expect(top(meters.frame(state, sources).monitor[0]!.peak)).toBeGreaterThan(-20);
  });
});
