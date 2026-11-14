import { describe, expect, it } from 'vitest';
import { RateLimiter, WRITE_INTERVAL_MS } from '../../src/lib/sync/limiter';
import { MAX_WRITES_PER_SECOND } from '../../src/lib/contract/protocol';
import { FakeClock } from './fakes';

function setup() {
  const clock = new FakeClock();
  const out: Array<{ t: number; path: string; value: unknown }> = [];
  const limiter = new RateLimiter(clock, (path, value) => out.push({ t: clock.now(), path, value }));
  return { clock, out, limiter };
}

describe('write rate limiter', () => {
  it('spaces writes of one control by 1000 / MAX_WRITES_PER_SECOND ms', () => {
    expect(WRITE_INTERVAL_MS).toBe(1000 / MAX_WRITES_PER_SECOND);
    expect(WRITE_INTERVAL_MS).toBe(20);
  });

  it('sends at most 50 writes per second to one control from a 1 kHz stream, and the last value', () => {
    const { clock, out, limiter } = setup();
    for (let i = 0; i < 1000; i++) {
      limiter.write('/f', i);
      clock.advance(1);
    }
    clock.advance(100);
    const inFirstSecond = out.filter((o) => o.t < 1000);
    expect(inFirstSecond.length).toBeLessThanOrEqual(MAX_WRITES_PER_SECOND);
    expect(inFirstSecond.length).toBeGreaterThanOrEqual(MAX_WRITES_PER_SECOND - 1);
    for (let i = 1; i < out.length; i++) expect(out[i]!.t - out[i - 1]!.t).toBeGreaterThanOrEqual(WRITE_INTERVAL_MS);
    expect(out.at(-1)!.value).toBe(999);
  });

  it('sends the trailing value when the interval ends, even with no further writes', () => {
    const { clock, out, limiter } = setup();
    limiter.write('/f', 1);
    clock.advance(5);
    limiter.write('/f', 2);
    limiter.write('/f', 3);
    expect(limiter.holds('/f')).toBe(true);
    expect(out.map((o) => o.value)).toEqual([1]);
    clock.advance(15);
    expect(out).toEqual([
      { t: 0, path: '/f', value: 1 },
      { t: 20, path: '/f', value: 3 },
    ]);
    expect(limiter.holds('/f')).toBe(false);
  });

  it('limits each control on its own', () => {
    const { out, limiter } = setup();
    limiter.write('/a', 1);
    limiter.write('/b', 1);
    limiter.write('/c', 1);
    expect(out.map((o) => o.path)).toEqual(['/a', '/b', '/c']);
  });
});
