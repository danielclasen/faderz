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
