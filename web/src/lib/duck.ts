// Duck groups on the console: what a group's state reads as, which groups
// a channel keys or is lowered by, and the DUCK key. The meter frames carry
// no duck telemetry yet, so everything here follows the state alone: a
// hold or mode on means the duck is in, auto means the key decides.

import type { DuckMode, DuckShape, DuckState, Shape, State } from './contract/types';
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

/** What a group does now, in words: held, latched, bypassed or keyed. */
export function lampText(d: DuckState): string {
  if (d.hold) return 'DUCK HELD';
  if (d.mode === 'on') return 'DUCK ON';
  if (d.mode === 'off') return 'BYPASS';
  return 'AUTO';
}

/** The duck is in whatever the key does: held or latched on. */
export const forced = (d: DuckState) => d.hold || d.mode === 'on';

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
 * through its mode keys, never by a tap here; a hold still ducks it.
 * While nothing commits no hold starts, as a hold sent late could
 * outlast the press.
 */
export class DuckPress {
  private pressing = false;
  private holding = false;
  private pressedAt = 0;

  constructor(
    private readonly mixer: MixerStore,
    private readonly id: string,
    private readonly name: () => string,
  ) {}

  /** This key holds the duck in now. */
  get held(): boolean {
    return this.holding;
  }

  press(now: number): void {
    if (this.pressing || this.mixer.locked) return;
    this.pressing = true;
    this.pressedAt = now;
    const hold = duckPath(this.id, 'hold');
    if (this.mixer.refuseIfHeld(hold) || this.mixer.notCommitting) return;
    this.holding = true;
    this.mixer.touch(hold);
    this.mixer.set(hold, true);
  }

  /** The press ended where it was made: a short one toggles the latch. */
  lift(now: number): void {
    if (!this.pressing) return;
    this.pressing = false;
    if (now - this.pressedAt < LATCH_MS) {
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
    this.end();
  }

  private end(): void {
    if (!this.holding) return;
    this.holding = false;
    const hold = duckPath(this.id, 'hold');
    // The server clears a hold its owner releases; while nothing commits
    // only the release goes, so the screen never shows it ended early.
    if (!this.mixer.notCommitting && this.mixer.shape?.ducks?.some((d) => d.id === this.id)) this.mixer.set(hold, false);
    this.mixer.release(hold);
  }
}
