import { describe, expect, it } from 'vitest';
import { loudnessNote } from '../../src/lib/loudnessNote';
import type { LoudnessHealth } from '../../src/lib/contract/types';

const health = (h: Partial<LoudnessHealth> = {}): LoudnessHealth => ({ id: 'pgm', gaps: 0, lost: 0, since: null, restarted: false, ...h });
/** 14:32 local time today, as the server sends it in UTC. */
const at = (() => {
  const d = new Date();
  d.setHours(14, 32, 5, 0);
  return d.toISOString();
})();

describe('loudnessNote', () => {
  it('says nothing of a measurement without gaps that started at the last reset', () => {
    expect(loudnessNote(undefined)).toEqual({ gaps: null, since: null, tip: '' });
    expect(loudnessNote(health({ since: at }))).toEqual({ gaps: null, since: null, tip: '' });
  });

  it('counts gaps and lost slices together, and says what a gap is and that lost ones are this replica alone', () => {
    const one = loudnessNote(health({ gaps: 1 }));
    expect(one.gaps).toBe('1 gap');
    expect(one.tip).toMatch(/^1 gap of 10 ms .*an input down, or audio this replica could not meter, such as when its host was short of CPU/);
    expect(one.tip).toContain('Integrated loudness and LRA carry on without them');
    expect(one.tip).toContain("can make its reading differ slightly from the other replicas'");
    expect(one.tip).not.toContain('on this replica only.');
    const both = loudnessNote(health({ gaps: 2, lost: 3 }));
    expect(both.gaps).toBe('5 gaps');
    expect(both.tip).toContain('3 of them on this replica only.');
    expect(both.since).toBeNull();
  });

  it('keeps a large count short and says how long it covers', () => {
    expect(loudnessNote(health({ gaps: 999 })).gaps).toBe('999 gaps');
    expect(loudnessNote(health({ gaps: 1050 })).gaps).toBe('1.0k gaps');
    expect(loudnessNote(health({ gaps: 6000 })).tip).toContain('about 60 s');
    expect(loudnessNote(health({ gaps: 12_345 })).gaps).toBe('12k gaps');
    expect(loudnessNote(health({ gaps: 12_345 })).tip).toContain('about 2 min');
  });

  it('shows when a restarted measurement started, in local time, and says earlier loudness is not in it', () => {
    const n = loudnessNote(health({ since: at, restarted: true }));
    expect(n.since).toBe('since 14:32');
    expect(n.gaps).toBeNull();
    expect(n.tip).toContain("this replica's measurement restarted; earlier programme loudness isn't included");
    // Not measuring yet: no start to name.
    expect(loudnessNote(health({ restarted: true })).since).toBeNull();
  });

  it('says both at once', () => {
    const n = loudnessNote(health({ gaps: 4, since: at, restarted: true }));
    expect([n.gaps, n.since]).toEqual(['4 gaps', 'since 14:32']);
    expect(n.tip).toMatch(/4 gaps of 10 ms .* Measuring since .*restarted/);
  });
});
