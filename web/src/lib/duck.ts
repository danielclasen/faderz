// Duck groups on the console: what a group's state and its readings in the
// meter frames read as, which groups a channel keys or is lowered by, and
// the DUCK key. A hold or mode on puts the duck in, auto lets the key
// decide; the frames tell how far it is in (envelope) and whether the key
// triggers it (engaged).

import type { DuckMeter, DuckMode, DuckShape, DuckState, Shape, State } from './contract/types';
import { fit } from './format';
import { STALE_MS } from './meters/layer';
import type { MeterStore } from './meters/store';
import type { MixerStore } from './state/mixer.svelte';
import { duckPath } from './state/paths';

/** A press shorter than this toggles mode between auto and on; a longer one is a momentary hold. */
export const LATCH_MS = 300;

export const DUCK_MODES: ReadonlyArray<{ id: DuckMode; key: string; label: string }> = [
  { id: 'auto', key: 'AUTO', label: 'auto: the key decides' },
  { id: 'on', key: 'ON', label: 'on: the duck is held in' },
  { id: 'off', key: 'OFF', label: 'off: bypassed, no duck whatever the key' },
];

/** Characters the activity text takes at most, a live reading such as "DUCK -40.0" included. */
export const LAMP_WIDTH = 10;

/**
 * What a group does now, in words. With its readings: while the envelope
 * is above 0, how far its deepest target is lowered, "DUCK -12.0"; else
 * held, latched, bypassed or auto, with KEY while the key triggers it.
 * Without readings, the state alone.
 */
export function lampText(d: DuckState, live: DuckMeter | null = null): string {
  if (live && live.envelope > 0) return `DUCK -${(deepestDepth(d) * live.envelope).toFixed(1)}`;
  if (d.hold) return 'DUCK HELD';
  if (d.mode === 'on') return 'DUCK ON';
  if (d.mode === 'off') return live?.engaged ? 'BYPASS KEY' : 'BYPASS';
  return live?.engaged ? 'KEY' : 'AUTO';
}

/** The lamp is lit: the duck is forced in or its envelope is above 0. */
export const lampLit = (d: DuckState, live: DuckMeter | null) => forced(d) || (live?.envelope ?? 0) > 0;

/** The duck is in whatever the key does: held or latched on. */
export const forced = (d: DuckState) => d.hold || d.mode === 'on';

/** Depth of the group's deepest target, dB; 0 without a target. */
export function deepestDepth(d: DuckState): number {
  let depth = 0;
  for (const m of Object.values(d.channels)) if (m.target && m.depth > depth) depth = m.depth;
  return depth;
}

/**
 * A group's readings while meter frames arrive, else null: a stale frame
 * is no reading. `now` is on the clock of MeterStore.ingest.
 */
export function liveDuck(meters: MeterStore, id: string, now: number): DuckMeter | null {
  if (meters.receivedAt <= 0 || now - meters.receivedAt >= STALE_MS) return null;
  return meters.duck(id) ?? null;
}

/**
 * How far `channel` is lowered now, dB, 0 or more: the deepest of its
 * groups' depth times envelope, as the engine attenuates a target by the
 * deepest of its groups. `live` gives a group's readings, or null.
 */
export function channelReduction(state: State, targets: readonly DuckShape[], channel: string, live: (id: string) => DuckMeter | null): number {
  let gr = 0;
  for (const g of targets) {
    const depth = state.ducks[g.id]?.channels[channel]?.depth ?? 0;
    gr = Math.max(gr, depth * (live(g.id)?.envelope ?? 0));
  }
  return gr;
}

/** Characters a strip's gain reduction takes. */
export const GR_WIDTH = 4;

/**
 * A gain reduction of `db` (0 or more) as a strip's duck chip shows it,
 * in GR_WIDTH characters: one decimal below 10 dB, "-3.5", whole dB from
 * there, " -12".
 */
export function reductionText(db: number): string {
  const tenths = Math.round(db * 10);
  return fit(tenths < 100 ? `-${(tenths / 10).toFixed(1)}` : `-${Math.round(db)}`, GR_WIDTH);
}

/** The groups a channel triggers and those that lower it. */
export function channelDucks(shape: Shape, state: State, channel: string): { triggers: DuckShape[]; targets: DuckShape[] } {
  const groups = shape.ducks ?? [];
  return {
    triggers: groups.filter((d) => state.ducks[d.id]?.channels[channel]?.trigger),
    targets: groups.filter((d) => state.ducks[d.id]?.channels[channel]?.target),
  };
}

/**
 * Why `channel` cannot be made a trigger of group `id` now, or null: it
 * is a target of the group, which would duck it under itself, or the
 * group has its 8 triggers. The server refuses both writes.
 */
export function triggerRefusal(shape: Shape, state: State, id: string, channel: string): string | null {
  const d = state.ducks[id];
  const name = (shape.ducks ?? []).find((g) => g.id === id)?.name ?? id;
  const ch = shape.channels.find((c) => c.id === channel)?.name ?? channel;
  if (!d || d.channels[channel]?.trigger) return null;
  if (d.channels[channel]?.target) return `${ch} is a target of ${name}: a channel cannot trigger the group that lowers it`;
  if (Object.values(d.channels).filter((m) => m.trigger).length >= MAX_TRIGGERS) return `${name} takes at most ${MAX_TRIGGERS} triggers`;
  return null;
}

/** Why `channel` cannot be made a target of group `id` now, or null. */
export function targetRefusal(shape: Shape, state: State, id: string, channel: string): string | null {
  const d = state.ducks[id];
  const name = (shape.ducks ?? []).find((g) => g.id === id)?.name ?? id;
  const ch = shape.channels.find((c) => c.id === channel)?.name ?? channel;
  if (!d || d.channels[channel]?.target || !d.channels[channel]?.trigger) return null;
  return `${ch} triggers ${name}: a channel cannot be lowered by the group it keys`;
}

/**
 * Why each channel cannot be made a trigger or a target of group `id`
 * now, by channel id, as triggerRefusal and targetRefusal say it: the
 * whole grid of a group from one count of its triggers, not one per cell.
 */
export function groupRefusals(shape: Shape, state: State, id: string): Map<string, { trigger: string | null; target: string | null }> {
  const out = new Map<string, { trigger: string | null; target: string | null }>();
  const d = state.ducks[id];
  if (!d) return out;
  const name = (shape.ducks ?? []).find((g) => g.id === id)?.name ?? id;
  const members = shape.channels.map((c) => ({ c, m: d.channels[c.id] }));
  const full = members.filter(({ m }) => m?.trigger).length >= MAX_TRIGGERS;
  for (const { c, m } of members) {
    out.set(c.id, {
      trigger: !m || m.trigger ? null : m.target ? `${c.name} is a target of ${name}: a channel cannot trigger the group that lowers it` : full ? `${name} takes at most ${MAX_TRIGGERS} triggers` : null,
      target: !m || m.target || !m.trigger ? null : `${c.name} triggers ${name}: a channel cannot be lowered by the group it keys`,
    });
  }
  return out;
}

/** Triggers a group takes at most (src/duck.rs MAX_TRIGGERS). */
export const MAX_TRIGGERS = 8;

/**
 * The DUCK key of a group, apart from its element. A press holds the duck
 * in at once: it touches the hold and sets it true. A press shorter than
 * LATCH_MS then toggles mode between auto and on, a latch that outlives
 * this screen; a longer one is momentary. Either way the end of the press
 * clears and releases the hold, and so does an interrupted press (abort).
 * The hold is owned: while another session holds it, a press holds
 * nothing and the notice names the holder, and a tap still toggles mode,
 * which has no owner. A bypassed group (mode off) changes mode only
 * through its mode keys, never by a tap here, and its hold starts only
 * once the press has lasted LATCH_MS: a stray tap during a break on open
 * microphones sends nothing, and a deliberate hold still ducks it.
 * While nothing commits no hold starts, as a hold sent late could
 * outlast the press.
 */
/**
 * Presses holding each group's hold now, per store: the tile, its pill and
 * the shortcut are separate keys of one hold, which ends when the last of
 * them lets go.
 */
const holders = new WeakMap<MixerStore, Map<string, number>>();

export class DuckPress {
  private pressing = false;
  private holding = false;
  private pressedAt = 0;
  /** The start of a bypassed group's hold is deferred to LATCH_MS into the press. */
  private deferred = false;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    private readonly mixer: MixerStore,
    private readonly id: string,
    private readonly name: () => string,
  ) {}

  /** This key holds the duck in now. */
  get held(): boolean {
    return this.holding;
  }

  private get count(): Map<string, number> {
    let m = holders.get(this.mixer);
    if (!m) holders.set(this.mixer, (m = new Map()));
    return m;
  }

  press(now: number): void {
    if (this.pressing || this.mixer.locked) return;
    this.pressing = true;
    this.pressedAt = now;
    this.deferred = false;
    if (this.mixer.value<DuckMode>(duckPath(this.id, 'mode')) !== 'off') return void this.start();
    this.deferred = true;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (this.pressing) this.start();
    }, LATCH_MS);
  }

  /**
   * Holds the duck in, unless writes are locked, nothing commits or
   * another session holds it, which the notice then names. Another key of
   * this screen already holding it only counts this one in.
   */
  private start(): boolean {
    const hold = duckPath(this.id, 'hold');
    if (this.mixer.locked || this.mixer.notCommitting || this.mixer.refuseIfHeld(hold)) return false;
    const n = this.count.get(this.id) ?? 0;
    this.count.set(this.id, n + 1);
    this.holding = true;
    if (n > 0) return true;
    this.mixer.touch(hold);
    this.mixer.set(hold, true);
    return true;
  }

  /**
   * The press ended where it was made: a short one toggles the latch,
   * unless `latch` is false, as for the keyboard shortcut, which only
   * ever holds. A bypassed group's press that lasted LATCH_MS holds and
   * lets go, even if a busy page has not run its deferred start yet.
   */
  lift(now: number, latch = true): void {
    if (!this.pressing) return;
    this.pressing = false;
    this.cancel();
    const short = now - this.pressedAt < LATCH_MS;
    if (this.deferred && !short && !this.holding) this.start();
    if (latch && short) {
      const path = duckPath(this.id, 'mode');
      const mode = this.mixer.value<DuckMode>(path);
      if (mode === 'off') this.mixer.notify('info', `${this.name()} is bypassed: choose AUTO or ON to arm it`, path);
      else this.mixer.set(path, mode === 'on' ? 'auto' : 'on');
    }
    this.end();
  }

  /** The press was interrupted (touch cancelled, capture or focus lost, page hidden, key gone): the hold ends. */
  abort(): void {
    if (!this.pressing) return;
    this.pressing = false;
    this.cancel();
    this.end();
  }

  private cancel(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  private end(): void {
    if (!this.holding) return;
    this.holding = false;
    const n = (this.count.get(this.id) ?? 1) - 1;
    if (n > 0) return void this.count.set(this.id, n);
    this.count.delete(this.id);
    const hold = duckPath(this.id, 'hold');
    // The server clears a hold its owner releases; while nothing commits
    // only the release goes, so the screen never shows it ended early. A
    // hold another session has taken since is not this one's to clear.
    if (!this.mixer.notCommitting && !this.mixer.ownedByOther(hold) && this.mixer.shape?.ducks?.some((d) => d.id === this.id)) this.mixer.set(hold, false);
    this.mixer.release(hold);
  }
}

/** Width of a duck tile in the masters band, px, and the gap after it. */
export const TILE_W = 141;
/** The least width the band's summary takes, px: one group's line with its name, what it lowers, its bar and its readouts. */
export const SUMMARY_MIN_W = 360;

/**
 * Whether the masters band shows its summary beside `groups` tiles in
 * `width` px: only with a fine pointer, outside the narrow layout where
 * the masters lie over the strips, and where the tiles leave it its
 * least width. Otherwise the band keeps its tiles alone.
 */
export function summaryFits(width: number, groups: number, coarse: boolean, narrow: boolean): boolean {
  return !coarse && !narrow && groups > 0 && width - groups * TILE_W >= SUMMARY_MIN_W;
}

/** What a group lowers and where, by name: its targets in shape order, then its destinations. */
export function duckReach(shape: Shape, d: DuckState): { targets: string; into: string } {
  const named = (ids: string[]) => ids.join(', ') || 'none';
  const targets = shape.channels.filter((c) => d.channels[c.id]?.target).map((c) => c.name);
  const into = [...shape.buses, ...shape.auxes, ...shape.mixMinus].filter((e) => d.destinations[e.id]).map((e) => e.name);
  return { targets: named(targets), into: named(into) };
}

/** Characters a key level readout takes, "-23.4" or "-inf". */
export const KEY_WIDTH = 5;

/** A group's key level as the summary shows it: dBFS with one decimal, "-inf" for silence, "--" while the group reports none. */
export function keyText(key: number | null): string {
  if (key === null) return fit('--', KEY_WIDTH);
  return fit(key <= -120 ? '-inf' : key.toFixed(1), KEY_WIDTH);
}

/** How much of the summary's reduction bar is lit: the reduction over the group's deepest depth, 0..1. */
export function reductionShare(gr: number, depth: number): number {
  return depth > 0 ? Math.min(1, Math.max(0, gr / depth)) : 0;
}
