import { describe, expect, it } from 'vitest';
import { applyImage, legMatrix } from '../../dev/mock/signals';
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
