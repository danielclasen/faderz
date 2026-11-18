import { describe, expect, it } from 'vitest';
import { KEY_WIDTH, SUMMARY_MIN_W, TILE_W, duckReach, keyShare, keyText, reductionShare, summaryFits, summaryReduction } from '../../src/lib/duck';
import type { DuckState } from '../../src/lib/contract/types';
import { exampleShape, initialState } from '../../dev/mock/shape';

const shape = exampleShape();
const comm = (patch: Partial<DuckState> = {}): DuckState => ({ ...initialState(shape).ducks['comm']!, ...patch });

describe('the duck band summary', () => {
  it('shows beside the tiles only where they leave it its least width, with a fine pointer and outside the narrow layout', () => {
    const just = 2 * TILE_W + SUMMARY_MIN_W;
    expect(summaryFits(just, 2, false, false)).toBe(true);
    expect(summaryFits(just - 1, 2, false, false)).toBe(false);
    // A third tile takes its width.
    expect(summaryFits(just, 3, false, false)).toBe(false);
    expect(summaryFits(4000, 2, true, false)).toBe(false);
    expect(summaryFits(4000, 2, false, true)).toBe(false);
    expect(summaryFits(4000, 0, false, false)).toBe(false);
    // Two rows a group: the band's height holds four.
    expect(summaryFits(4000, 4, false, false)).toBe(true);
    expect(summaryFits(4000, 5, false, false)).toBe(false);
  });

  it('still shows beside four tiles in the example shape\'s masters', () => {
    // The example shape's masters, and so the band, are 776 px wide.
    expect(summaryFits(776, 4, false, false)).toBe(true);
    expect(summaryFits(776, 5, false, false)).toBe(false);
  });

  it('names the outputs a group lowers and its targets, in shape order, and says when there are none', () => {
    const d = comm();
    const targets = shape.channels.filter((c) => d.channels[c.id]?.target).map((c) => c.name);
    const into = [...shape.buses, ...shape.auxes, ...shape.mixMinus].filter((e) => d.destinations[e.id]).map((e) => e.name);
    expect(targets.length).toBeGreaterThan(0);
    expect(duckReach(shape, d)).toEqual({ targets: targets.join(', '), into: into.join(', ') });
    const none = comm({
      channels: Object.fromEntries(Object.entries(d.channels).map(([id, m]) => [id, { ...m, target: false }])),
      destinations: Object.fromEntries(Object.keys(d.destinations).map((id) => [id, false])),
    });
    expect(duckReach(shape, none)).toEqual({ targets: 'no targets', into: 'no outputs' });
  });

  it('reads the key level in a fixed width, -inf from -100 dBFS down as other level readouts', () => {
    for (const key of [-23.44, -99.9, -100, -110, -119.9, -120, -3, 0, null]) expect(keyText(key)).toHaveLength(KEY_WIDTH);
    expect(keyText(-23.44).trim()).toBe('-23.4');
    expect(keyText(-99.9).trim()).toBe('-99.9');
    for (const key of [-100, -110, -119.9, -120]) expect(keyText(key).trim()).toBe('-inf');
    expect(keyText(null).trim()).toBe('--');
  });

  it('reads the reduction with one decimal in 5 characters, as the tile lamp', () => {
    expect(summaryReduction(15)).toBe('-15.0');
    expect(summaryReduction(3.54)).toBe(' -3.5');
    expect(summaryReduction(0)).toBe('  0.0');
  });

  it('draws every reduction bar on one scale, and the key on the threshold\'s', () => {
    // -15 and -12 dB on a 15 dB scale: equal bars only for equal reductions.
    expect(reductionShare(15, 15)).toBe(1);
    expect(reductionShare(12, 15)).toBeCloseTo(0.8, 10);
    expect(reductionShare(20, 15)).toBe(1);
    expect(reductionShare(0, 15)).toBe(0);
    expect(reductionShare(5, 0)).toBe(0);
    expect(keyShare(-30)).toBe(0.5);
    expect(keyShare(-90)).toBe(0);
    expect(keyShare(6)).toBe(1);
    expect(keyShare(null)).toBe(0);
  });
});
