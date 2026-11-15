import { describe, expect, it } from 'vitest';
import { levelText } from '../../src/lib/format';

describe('level readouts', () => {
  it('fit five characters in LUFS and in LU, whatever the value', () => {
    for (const target of [undefined, -14, -23, -31]) {
      for (const v of [null, undefined, -120, -100, -99.95, -70, -23, 0, 29.9, 30]) {
        expect(levelText(v, target).length, `${v} against ${target}`).toBe(5);
      }
    }
  });

  it('read "-inf" at or below -100 in LU as in LUFS', () => {
    expect(levelText(-120)).toBe(' -inf');
    expect(levelText(-120, -14)).toBe(' -inf');
    expect(levelText(-100, -23)).toBe(' -inf');
    expect(levelText(-99.9, -14)).toBe('-85.9');
  });

  it('sign a value relative to the target', () => {
    expect(levelText(-22, -23)).toBe(' +1.0');
    expect(levelText(-24.04, -23)).toBe(' -1.0');
    expect(levelText(null, -23)).toBe(' --.-');
  });
});
