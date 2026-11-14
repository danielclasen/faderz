import { describe, expect, it } from 'vitest';
import { CounterRates, engineLamp } from '../../src/lib/counters';

describe('CounterRates', () => {
  it('ignores totals present when the screen opened', () => {
    const r = new CounterRates();
    expect(r.update({ deadline_miss_total: 42, input_late_blocks_total: 98 }, 0)).toEqual([]);
    expect(r.update({ deadline_miss_total: 42, input_late_blocks_total: 98 }, 1000)).toEqual([]);
  });

  it('warns while a counter grew within the window and calms down after it', () => {
    const r = new CounterRates();
    r.update({ deadline_miss_total: 42, afv_late_total: 0 }, 0);
    expect(r.update({ deadline_miss_total: 43, afv_late_total: 0 }, 1000)).toEqual(['deadline_miss_total']);
    expect(r.update({ deadline_miss_total: 43, afv_late_total: 0 }, 10_500)).toEqual(['deadline_miss_total']);
    expect(r.update({ deadline_miss_total: 43, afv_late_total: 0 }, 11_500)).toEqual([]);
  });

  it('does not warn on a counter that went down (a reset)', () => {
    const r = new CounterRates();
    r.update({ command_late_total: 13 }, 0);
    expect(r.update({ command_late_total: 2 }, 1000)).toEqual([]);
    expect(r.update({ command_late_total: 3 }, 2000)).toEqual(['command_late_total']);
  });

  it('takes the totals of another replica as a new baseline, not as growth', () => {
    // After a failover the counters are another engine's totals.
    const r = new CounterRates();
    r.update({ deadline_miss_total: 4, input_late_blocks_total: 7 }, 0, 'r1');
    expect(r.update({ deadline_miss_total: 90, input_late_blocks_total: 300 }, 1000, 'r2')).toEqual([]);
    expect(r.update({ deadline_miss_total: 91, input_late_blocks_total: 300 }, 2000, 'r2')).toEqual(['deadline_miss_total']);
  });
});

describe('engineLamp', () => {
  it('gives OK and every WARN count one width, so the lamp never changes size', () => {
    const width = engineLamp(0).length;
    expect(engineLamp(0).trim()).toBe('ENGINE OK');
    for (const n of [1, 9, 10, 42, 99, 100, 1000]) expect(engineLamp(n), String(n)).toHaveLength(width);
    expect(engineLamp(12).trim()).toBe('ENGINE WARN 12');
    expect(engineLamp(150).trim()).toBe('ENGINE WARN 99+');
  });
});
