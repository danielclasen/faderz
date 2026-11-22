// Engine counters are totals since the engine started. An operator needs
// to know whether something is going wrong now, so the lamp follows the
// rise of each counter within the last window, not its total, and only
// of the counters whose rise is a fault. Each counter's name for people
// and what it means come from the contract (counters.schema.json), so the
// console and the API documentation say the same.

import schema from '../../../api/ws/counters.schema.json' with { type: 'json' };
import type { InputHealth } from './contract/types';
import { fit } from './format';

export const WARN_WINDOW_MS = 10_000;

/** Width of the lamp text: "WARN IN VT 2 +2 more" fills it; StatusBar's .lamp box is as wide. */
const LAMP_W = 20;

/**
 * The counters whose rise is a fault now: audio was lost or late, a change
 * was not applied, or an output or the clock failed. The others count
 * events expected at times, such as while a replica starts, or follow one
 * of these, and never light the lamp.
 */
export const FAULTS: ReadonlySet<string> = new Set([
  'deadline_miss_total',
  'overrun_skipped_samples_total',
  'clock_steps_total',
  'input_late_blocks_total',
  'input_too_late_blocks_total',
  'input_silenced_blocks_total',
  'output_failed_writes_total',
  'command_late_total',
  'generation_late_total',
  'strip_refused_total',
]);

/** The counters that count an input's blocks: a rise of one is told by input. */
export const INPUT_COUNTERS: ReadonlySet<string> = new Set(['input_late_blocks_total', 'input_too_late_blocks_total', 'input_silenced_blocks_total']);

export interface CounterInfo {
  /** Its name for people. */
  label: string;
  /** What it counts and what a rise means. */
  help: string;
  /** A rise is a fault now and lights the lamp. */
  fault: boolean;
}

const KNOWN: Record<string, { title: string; description: string }> = schema.properties;

/** Every counter the contract names, in its order. */
export const COUNTER_NAMES: readonly string[] = Object.keys(KNOWN);
const RANK = new Map(COUNTER_NAMES.map((n, i) => [n, i]));
/** A counter's place in the contract's order; one it does not name after them all. */
const rank = (name: string) => RANK.get(name) ?? COUNTER_NAMES.length;

/** What the contract says of counter `name`; one it does not name shows by its key. */
export function counterInfo(name: string): CounterInfo {
  const known = KNOWN[name];
  return known
    ? { label: known.title, help: known.description, fault: FAULTS.has(name) }
    : { label: name.replace(/_total$/, ''), help: '', fault: false };
}

/** Each key's rise within WARN_WINDOW_MS, from the values it was read at. */
class Window {
  /** Readings of each key, oldest first: the newest at or before the window's start, then those in it. */
  private readonly seen = new Map<string, Array<[number, number]>>();

  clear(): void {
    this.seen.clear();
  }

  /** Records `value` of `key` at `now`; returns its rise within the window before `now`. */
  rise(key: string, value: number, now: number): number {
    let s = this.seen.get(key);
    // The first reading is a baseline: a total from before this screen
    // opened is not "now". A total that went down was reset (engine
    // restart): it starts a new baseline.
    if (!s || value < s.at(-1)![1]) {
      this.seen.set(key, [[now, value]]);
      return 0;
    }
    s.push([now, value]);
    while (s.length > 1 && s[1]![0] <= now - WARN_WINDOW_MS) s.shift();
    return value - s[0]![1];
  }

  /** Forgets the keys not read in the last update. */
  keep(keys: Set<string>): void {
    for (const k of this.seen.keys()) if (!keys.has(k)) this.seen.delete(k);
  }
}

/** An input that delivers nothing usable now, and why. */
export interface InputFault {
  channel: string;
  /** Its flow delivers nothing in time: silenced until it moves again. */
  pruned: boolean;
  /** The replica set took it down: silenced on every replica. */
  down: boolean;
  /** The engine's reason it does not read, such as its flow not found or its head off the MXL clock; null while none. */
  error: string | null;
  /** Its late and too-late blocks within the window. */
  late: number;
  tooLate: number;
}

/** Why `f` delivers nothing usable, in words. */
export function inputReasons(f: InputFault): string[] {
  const s = WARN_WINDOW_MS / 1000;
  const blocks = (n: number) => `${n} ${n === 1 ? 'block' : 'blocks'}`;
  return [
    ...(f.pruned ? ['its flow delivers no audio in time, so it is silenced until the flow moves again'] : []),
    ...(f.down ? ['the replica set took it down, so it is silenced on every replica'] : []),
    ...(f.error !== null ? [`the engine reports: ${f.error}`] : []),
    ...(f.tooLate > 0 ? [`${blocks(f.tooLate)} came too late to use in the last ${s} s`] : []),
    ...(f.late > 0 ? [`${blocks(f.late)} came late in the last ${s} s`] : []),
  ];
}

/** The inputs among `faults` a rise of input counter `name` comes from. */
export function inputsBehind(name: string, faults: InputFault[]): InputFault[] {
  if (name === 'input_late_blocks_total') return faults.filter((f) => f.late > 0);
  if (name === 'input_too_late_blocks_total') return faults.filter((f) => f.tooLate > 0);
  return INPUT_COUNTERS.has(name) ? faults : [];
}

export interface Assessment {
  /** Each counter's rise within the window. */
  rises: Record<string, number>;
  /** The fault counters that rose within the window, in the contract's order. */
  warnings: string[];
  /** The inputs at fault now: those that deliver nothing first, then those losing blocks, each in shape order. */
  inputs: InputFault[];
}

export class CounterRates {
  private readonly counters = new Window();
  private readonly inputs = new Window();
  private replica: string | null = null;

  /**
   * What `values` and `inputs` of `replica`, read at `now`, say is wrong
   * now. Totals of another replica are a new baseline: after a failover
   * they are another engine's, not a rise.
   */
  update(values: Record<string, number>, now: number, replica: string | null = null, inputs: InputHealth[] = []): Assessment {
    if (replica !== this.replica) {
      this.counters.clear();
      this.inputs.clear();
      this.replica = replica;
    }
    const rises: Record<string, number> = {};
    for (const [name, value] of Object.entries(values)) rises[name] = this.counters.rise(name, value, now);
    this.counters.keep(new Set(Object.keys(values)));
    const warnings = Object.keys(rises)
      .filter((n) => rises[n]! > 0 && FAULTS.has(n))
      .sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
    const keys = new Set<string>();
    const faults: InputFault[] = [];
    for (const i of inputs) {
      keys.add(`${i.channel}/late`).add(`${i.channel}/tooLate`);
      const late = this.inputs.rise(`${i.channel}/late`, i.lateBlocks, now);
      const tooLate = this.inputs.rise(`${i.channel}/tooLate`, i.tooLateBlocks, now);
      // A detached input is routed to no flow on purpose: silent, not at fault.
      if (i.state === 'detached') continue;
      // An older replica sends no error, during a rolling upgrade.
      const f = { channel: i.channel, pruned: i.state === 'pruned', down: !i.up, error: i.error ?? null, late, tooLate };
      if (f.pruned || f.down || f.error !== null || late > 0 || tooLate > 0) faults.push(f);
    }
    this.inputs.keep(keys);
    // An input that delivers nothing at all first, then one losing blocks, each in shape order.
    const weight = (f: InputFault) => (f.pruned || f.down || f.error !== null ? 0 : f.tooLate > 0 ? 1 : 2);
    faults.sort((a, b) => weight(a) - weight(b));
    return { rises, warnings, inputs: faults };
  }
}

/**
 * The lamp's text, at one width for every case: OK; WARN IN naming the
 * first input at fault, and how many more inputs there are; or WARN with
 * the number of fault counters that rose.
 */
export function engineLamp(warnings: number, inputs: string[] = []): string {
  if (inputs.length > 0) {
    const more = inputs.length > 1 ? ` +${inputs.length - 1} more` : '';
    // A long name is cut, so the count of the others stays; the tooltip has it whole.
    return fit(`WARN IN ${inputs[0]!.slice(0, LAMP_W - 8 - more.length)}${more}`, LAMP_W);
  }
  return fit(warnings === 0 ? 'ENGINE OK' : `ENGINE WARN ${warnings > 99 ? '99+' : warnings}`, LAMP_W);
}
