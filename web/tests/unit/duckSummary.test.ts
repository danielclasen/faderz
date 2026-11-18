import { describe, expect, it } from 'vitest';
import { KEY_WIDTH, SUMMARY_MIN_W, TILE_W, duckReach, keyText, reductionShare, summaryFits } from '../../src/lib/duck';
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
  });

  it('names what a group lowers and where, in shape order', () => {
    const d = comm();
    const targets = shape.channels.filter((c) => d.channels[c.id]?.target).map((c) => c.name);
    const into = [...shape.buses, ...shape.auxes, ...shape.mixMinus].filter((e) => d.destinations[e.id]).map((e) => e.name);
    expect(targets.length).toBeGreaterThan(0);
    expect(duckReach(shape, d)).toEqual({ targets: targets.join(', '), into: into.join(', ') });
    const none = comm({
      channels: Object.fromEntries(Object.entries(d.channels).map(([id, m]) => [id, { ...m, target: false }])),
      destinations: Object.fromEntries(Object.keys(d.destinations).map((id) => [id, false])),
    });
    expect(duckReach(shape, none)).toEqual({ targets: 'none', into: 'none' });
  });

  it('reads the key level in a fixed width, silence and no reading included', () => {
    for (const key of [-23.44, -120, -3, 0, null]) expect(keyText(key)).toHaveLength(KEY_WIDTH);
    expect(keyText(-23.44).trim()).toBe('-23.4');
    expect(keyText(-120).trim()).toBe('-inf');
    expect(keyText(null).trim()).toBe('--');
  });

  it('lights the reduction bar by the reduction over the deepest depth', () => {
    expect(reductionShare(7.5, 15)).toBe(0.5);
    expect(reductionShare(15, 15)).toBe(1);
    expect(reductionShare(20, 15)).toBe(1);
    expect(reductionShare(0, 15)).toBe(0);
    expect(reductionShare(5, 0)).toBe(0);
  });
});
