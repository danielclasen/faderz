import { describe, expect, it } from 'vitest';
import { loudnessNote } from '../../src/lib/loudnessNote';
import type { LoudnessHealth } from '../../src/lib/contract/types';

const health = (h: Partial<LoudnessHealth> = {}): LoudnessHealth => ({ id: 'pgm', gaps: 0, lost: 0, since: null, restarted: false, ...h });
/** Noon on 20 Nov, local time. */
const now = new Date(2026, 10, 20, 12, 0, 0);
const local = (d: number, h: number, m: number) => new Date(2026, 10, d, h, m, 5).toISOString();

describe('loudnessNote', () => {
  it('says nothing of a measurement without gaps that did not restart', () => {
    expect(loudnessNote(undefined, now)).toEqual({ gaps: null, since: null, tip: '' });
    expect(loudnessNote(health({ since: local(20, 9, 0) }), now)).toEqual({ gaps: null, since: null, tip: '' });
  });

  it('counts gaps and lost slices together, in words an operator reads, and says gaps on this instance alone set it apart', () => {
    const one = loudnessNote(health({ gaps: 1 }), now);
    expect(one.gaps).toBe('1 gap');
    expect(one.tip).toMatch(/^1 gap of 10 ms in this measurement, under 1 s: an input was down, or this mixer instance could not measure the audio, for example while the server was overloaded/);
    expect(one.tip).toContain('Integrated loudness and LRA carry on without them');
    expect(one.tip).toContain("can make its reading differ slightly from the other instances'");
    expect(one.tip).not.toContain('on this mixer instance only.');
    expect(one.tip).not.toMatch(/replica|CPU/);
    const both = loudnessNote(health({ gaps: 2, lost: 3 }), now);
    expect(both.gaps).toBe('5 gaps');
    expect(both.tip).toContain('3 of them on this mixer instance only.');
    expect(both.since).toBeNull();
  });

  it('keeps a large count to at most four characters and says how long it covers', () => {
    expect(loudnessNote(health({ gaps: 999 }), now).gaps).toBe('999 gaps');
    expect(loudnessNote(health({ gaps: 1050 }), now).gaps).toBe('1k gaps');
    expect(loudnessNote(health({ gaps: 6000 }), now).tip).toContain('about 60 s');
    expect(loudnessNote(health({ gaps: 12_345 }), now).gaps).toBe('12k gaps');
    expect(loudnessNote(health({ gaps: 12_345 }), now).tip).toContain('about 2 min');
  });

  it('shows when a restarted measurement started, in local time, with the date when not today', () => {
    const today = loudnessNote(health({ since: local(20, 14, 32), restarted: true }), now);
    expect(today.since).toBe('since 14:32');
    expect(today.gaps).toBeNull();
    expect(today.tip).toBe("This mixer instance's measurement restarted at 14:32; earlier programme loudness isn't included.");
    expect(loudnessNote(health({ since: local(18, 7, 5), restarted: true }), now).since).toBe('since 18 Nov 07:05');
    // Not measuring yet: no start to name.
    expect(loudnessNote(health({ restarted: true }), now).since).toBeNull();
  });

  it('says both at once', () => {
    const n = loudnessNote(health({ gaps: 4, since: local(20, 14, 32), restarted: true }), now);
    expect([n.gaps, n.since]).toEqual(['4 gaps', 'since 14:32']);
    expect(n.tip).toMatch(/4 gaps of 10 ms .* restarted at 14:32/);
  });
});

describe('a counters message without the loudness report', () => {
  it('leaves the notes empty, as an older replica sends it during a rolling upgrade', async () => {
    const { INITIAL_STATUS, MixerStore } = await import('../../src/lib/state/mixer.svelte');
    const { exampleShape, initialState } = await import('../../dev/mock/shape');
    const s = new MixerStore();
    s.attach({ post: () => {} });
    const shape = exampleShape();
    const status = { ...INITIAL_STATUS, link: 'Live' as const };
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status });
    const replica = { id: 'r1', role: 'leader' as const };
    const counters = (extra: object) => ({ type: 'patch' as const, input: 1, changes: [], owners: [], counters: { replica, values: {}, inputs: [], ...extra } });
    s.apply(counters({ loudness: [health({ gaps: 2 })] }) as never);
    expect(loudnessNote(s.loudnessHealth.pgm, now).gaps).toBe('2 gaps');
    s.apply(counters({}) as never);
    expect(s.loudnessHealth).toEqual({});
    expect(loudnessNote(s.loudnessHealth.pgm, now)).toEqual({ gaps: null, since: null, tip: '' });
  });
});
