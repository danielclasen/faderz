import { describe, expect, it } from 'vitest';
import { COUNTER_NAMES, CounterRates, FAULTS, INPUT_COUNTERS, WARN_WINDOW_MS, counterInfo, engineLamp, inputReasons, inputsBehind } from '../../src/lib/counters';
import schema from '../../../api/ws/counters.schema.json' with { type: 'json' };
import type { InputHealth } from '../../src/lib/contract/types';

const input = (channel: string, extra: Partial<InputHealth> = {}): InputHealth => ({ channel, state: 'attached', up: true, lateBlocks: 0, tooLateBlocks: 0, ...extra });

describe('CounterRates', () => {
  it('ignores totals present when the screen opened', () => {
    const r = new CounterRates();
    expect(r.update({ deadline_miss_total: 42, input_late_blocks_total: 98 }, 0).warnings).toEqual([]);
    expect(r.update({ deadline_miss_total: 42, input_late_blocks_total: 98 }, 1000).warnings).toEqual([]);
  });

  it('warns while a fault counter rose within the window, with the rise, and calms down after it', () => {
    const r = new CounterRates();
    r.update({ deadline_miss_total: 42 }, 0);
    expect(r.update({ deadline_miss_total: 43 }, 1000)).toMatchObject({ warnings: ['deadline_miss_total'], rises: { deadline_miss_total: 1 } });
    expect(r.update({ deadline_miss_total: 45 }, 2000)).toMatchObject({ warnings: ['deadline_miss_total'], rises: { deadline_miss_total: 3 } });
    // The rise of 1 at 1 s leaves the window at 11 s, the one of 2 at 2 s at 12 s.
    expect(r.update({ deadline_miss_total: 45 }, 1000 + WARN_WINDOW_MS + 500).rises).toEqual({ deadline_miss_total: 2 });
    expect(r.update({ deadline_miss_total: 45 }, 2000 + WARN_WINDOW_MS + 500)).toMatchObject({ warnings: [], rises: { deadline_miss_total: 0 } });
  });

  it('counts a rise read once a second as one within the window, whatever the total', () => {
    const r = new CounterRates();
    let total = 1_000_000;
    r.update({ input_silenced_blocks_total: total }, 0);
    for (let t = 1000; t <= 30_000; t += 1000) {
      total += 200;
      const { rises } = r.update({ input_silenced_blocks_total: total }, t);
      expect(rises['input_silenced_blocks_total'], `${t}`).toBe(Math.min(t, WARN_WINDOW_MS) / 1000 * 200);
    }
  });

  it('never warns on a counter whose rise is expected at times', () => {
    const r = new CounterRates();
    const quiet = { blocks_without_core_total: 0, blocks_withheld_total: 0, meter_dropped_total: 0, resends_total: 0 };
    r.update(quiet, 0);
    const a = r.update({ blocks_without_core_total: 900, blocks_withheld_total: 900, meter_dropped_total: 5, resends_total: 1 }, 1000);
    expect(a.warnings).toEqual([]);
    expect(a.rises['blocks_without_core_total']).toBe(900);
  });

  it('lists the fault counters in the contract order', () => {
    const r = new CounterRates();
    const zero = { strip_refused_total: 0, deadline_miss_total: 0, command_late_total: 0 };
    r.update(zero, 0);
    expect(r.update({ strip_refused_total: 1, deadline_miss_total: 1, command_late_total: 1 }, 1000).warnings).toEqual([
      'deadline_miss_total',
      'command_late_total',
      'strip_refused_total',
    ]);
  });

  it('does not warn on a counter that went down (a reset), and counts from the reset on', () => {
    const r = new CounterRates();
    r.update({ command_late_total: 13 }, 0);
    expect(r.update({ command_late_total: 2 }, 1000).warnings).toEqual([]);
    expect(r.update({ command_late_total: 3 }, 2000)).toMatchObject({ warnings: ['command_late_total'], rises: { command_late_total: 1 } });
  });

  it('takes the totals of another replica as a new baseline, not as a rise', () => {
    // After a failover the counters are another engine's totals.
    const r = new CounterRates();
    r.update({ deadline_miss_total: 4, input_late_blocks_total: 7 }, 0, 'r1', [input('ch1', { lateBlocks: 7 })]);
    const failover = r.update({ deadline_miss_total: 90, input_late_blocks_total: 300 }, 1000, 'r2', [input('ch1', { lateBlocks: 300 })]);
    expect(failover).toMatchObject({ warnings: [], inputs: [] });
    expect(r.update({ deadline_miss_total: 91, input_late_blocks_total: 300 }, 2000, 'r2').warnings).toEqual(['deadline_miss_total']);
  });

  it('names an input that delivers nothing, and why, from its state', () => {
    const r = new CounterRates();
    const inputs = [input('ch1'), input('mvw', { state: 'pruned' }), input('ch3', { up: false }), input('ch4', { state: 'detached', up: false })];
    const a = r.update({ input_silenced_blocks_total: 0 }, 0, 'r1', inputs);
    // A pruned or downed input is at fault from the first reading on: its state is now, not a total.
    expect(a.inputs.map((f) => f.channel)).toEqual(['mvw', 'ch3']);
    expect(inputReasons(a.inputs[0]!).join()).toMatch(/flow delivers no audio in time/);
    expect(inputReasons(a.inputs[1]!).join()).toMatch(/replica set took it down/);
    // Both are behind the silenced blocks, neither behind the late ones.
    expect(inputsBehind('input_silenced_blocks_total', a.inputs)).toHaveLength(2);
    expect(inputsBehind('input_late_blocks_total', a.inputs)).toEqual([]);
    expect(inputsBehind('deadline_miss_total', a.inputs)).toEqual([]);
  });

  it('names an input that delivers nothing before one that loses a block now and then', () => {
    const r = new CounterRates();
    r.update({}, 0, 'r1', [input('ch1'), input('st3', { state: 'pruned' }), input('ch2'), input('ch9')]);
    const a = r.update({}, 1000, 'r1', [input('ch1', { lateBlocks: 1 }), input('st3', { state: 'pruned' }), input('ch2', { tooLateBlocks: 1 }), input('ch9', { up: false })]);
    expect(a.inputs.map((f) => f.channel)).toEqual(['st3', 'ch9', 'ch2', 'ch1']);
    expect(engineLamp(a.warnings.length, a.inputs.map((f) => f.channel)).trim()).toBe('WARN IN st3 +3 more');
  });

  it('names an input whose late blocks rose within the window, and only then', () => {
    const r = new CounterRates();
    r.update({ input_late_blocks_total: 50 }, 0, 'r1', [input('ch1', { lateBlocks: 50 }), input('ch2', { lateBlocks: 0 })]);
    const a = r.update({ input_late_blocks_total: 53 }, 1000, 'r1', [input('ch1', { lateBlocks: 50 }), input('ch2', { lateBlocks: 2, tooLateBlocks: 1 })]);
    expect(a.warnings).toEqual(['input_late_blocks_total']);
    expect(a.inputs).toEqual([{ channel: 'ch2', pruned: false, down: false, late: 2, tooLate: 1 }]);
    expect(inputReasons(a.inputs[0]!)).toEqual(['1 block came too late to use in the last 10 s', '2 blocks came late in the last 10 s']);
    expect(inputsBehind('input_late_blocks_total', a.inputs).map((f) => f.channel)).toEqual(['ch2']);
    const later = r.update({ input_late_blocks_total: 53 }, 1000 + WARN_WINDOW_MS + 1, 'r1', [input('ch1', { lateBlocks: 50 }), input('ch2', { lateBlocks: 2, tooLateBlocks: 1 })]);
    expect(later).toMatchObject({ warnings: [], inputs: [] });
  });
});

describe('counterInfo', () => {
  it('shows each counter by the title and description of the contract, which names every fault and input counter', () => {
    const props: Record<string, { title: string; description: string }> = schema.properties;
    expect(COUNTER_NAMES).toEqual(Object.keys(props));
    for (const [name, p] of Object.entries(props)) expect(counterInfo(name), name).toMatchObject({ label: p.title, help: p.description });
    for (const n of [...FAULTS, ...INPUT_COUNTERS]) expect(COUNTER_NAMES, n).toContain(n);
  });


  it('names a counter for people from the contract, and one it does not know by its key', () => {
    expect(counterInfo('deadline_miss_total')).toMatchObject({ label: 'Late output blocks', fault: true });
    expect(counterInfo('meter_dropped_total').fault).toBe(false);
    // A near miss of an input switch did not affect the audio: it lights no lamp.
    expect(counterInfo('generation_past_margin_total')).toMatchObject({ label: 'Tight input switches', fault: false });
    // A late tally is the switcher's, not this replica's: it lights no lamp.
    expect(counterInfo('afv_late_total').fault).toBe(false);
    expect(counterInfo('something_new_total')).toEqual({ label: 'something_new', help: '', fault: false });
  });
});

describe('engineLamp', () => {
  it('gives OK, every WARN count and every named input one width, so the lamp never changes size', () => {
    const width = engineLamp(0).length;
    expect(engineLamp(0).trim()).toBe('ENGINE OK');
    for (const n of [1, 9, 10, 42, 99, 100, 1000]) expect(engineLamp(n), String(n)).toHaveLength(width);
    expect(engineLamp(12).trim()).toBe('ENGINE WARN 12');
    expect(engineLamp(150).trim()).toBe('ENGINE WARN 99+');
    // IN says it is an input; "more" says the count is of inputs, not a rise.
    expect(engineLamp(1, ['mvw']).trim()).toBe('WARN IN mvw');
    expect(engineLamp(0, ['VT 2', 'ch3', 'ch4']).trim()).toBe('WARN IN VT 2 +2 more');
    for (const names of [['mvw'], ['a-very-long-channel-name'], ['a-very-long-channel-name', 'b']]) {
      expect(engineLamp(1, names), names.join()).toHaveLength(width);
    }
    // A cut name keeps the count of the others readable.
    expect(engineLamp(1, ['a-very-long-channel-name', 'b']).trim()).toMatch(/\+1 more$/);
  });
});
