// Latest meter frame, as the flattened values the sync worker transfers
// (meters/flat.ts). Kept outside Svelte reactivity: canvases read it from
// one animation-frame loop instead of re-rendering at 25 Hz.

import { SvelteSet } from 'svelte/reactivity';
import { deriveMeterLayout } from '../contract/meterFrame';
import type { BusMeter, Shape } from '../contract/types';
import { meterSlots, slotKey, type MeterGroup, type MeterSlot } from './flat';

export type { MeterGroup } from './flat';

export interface MeterReading {
  peak: number[];
  /** Empty for a monitor output, which carries peak only. */
  truePeak: number[];
  /** Bus, aux and mix-minus only. */
  rms?: number[];
  /** Channels: momentary loudness at the meter tap. */
  momentary?: number | null;
  loudness?: BusMeter['loudness'];
  correlation?: number | null;
}

/** What a loudness measurement shows of its reset: NaN for null. */
interface LoudSeen {
  integrated: number;
  lra: number;
  maxMomentary: number;
  maxShortTerm: number;
  lraStable: boolean;
}

/** Frames kept for maxima over a display period: 0.64 s at 25 frames per second. */
const HISTORY = 16;

export class MeterStore {
  private slots = new Map<string, MeterSlot>();
  private fingerprint: number | null = null;
  /** Elements whose true peak is shown, with their slots: buses, auxes, mix-minus outputs and channels with a direct out. */
  private truePeaks: Array<[string, MeterSlot]> = [];
  private directOuts = new Set<string>();
  private values: Float32Array | null = null;
  private readings = new Map<string, MeterReading>();
  /** The latest frames, oldest first, with the time each arrived. */
  private history: Array<{ at: number; values: Float32Array }> = [];
  /** Highest true peak per slot key since it was last cleared, dBTP. */
  private readonly tpMax = new Map<string, number>();
  /** Loudness maxima and LRA state per slot key in the last frame of `loudReplica`. */
  private readonly loudSeen = new Map<string, LoudSeen>();
  private loudReplica: string | null = null;
  /** Replica that measured the latest frame. */
  replica: string | null = null;
  receivedAt = 0;
  /** True peak above which an element latches an over, dBTP (State.metering.zones.truePeakMax). */
  truePeakMax = -1;
  /**
   * Slot keys (group/id) whose true peak went above truePeakMax in any
   * ingested frame since the key was last cleared. Reactive: it changes
   * only when an over latches or is cleared.
   */
  readonly overs = new SvelteSet<string>();

  /**
   * Meter frames follow the shape of the last snapshot. Every snapshot
   * sets it, also after a reconnect or a rewind: latched overs and true
   * peak maxima clear only when the meter layout changes.
   */
  setShape(shape: Shape): void {
    const layout = deriveMeterLayout(shape);
    if (layout.fingerprint !== this.fingerprint) {
      this.fingerprint = layout.fingerprint;
      this.slots = meterSlots(layout);
      this.overs.clear();
      this.tpMax.clear();
      this.loudSeen.clear();
    }
    this.directOuts = new Set(shape.outputs.map((o) => o.source));
    this.truePeaks = [...this.slots].filter(([key, slot]) => this.showsTruePeak(key, slot));
    const shown = new Set(this.truePeaks.map(([key]) => key));
    for (const key of [...this.overs]) if (!shown.has(key)) this.overs.delete(key);
    for (const key of [...this.tpMax.keys()]) if (!shown.has(key)) this.tpMax.delete(key);
    this.clear();
  }

  /**
   * A channel's true peak is measured at its direct-out tap, so it is
   * shown only for a channel that feeds an output; a monitor output
   * carries none.
   */
  private showsTruePeak(key: string, slot: MeterSlot): boolean {
    if (slot.kind === 'monitor') return false;
    return slot.kind !== 'channel' || this.directOuts.has(key.slice('channels/'.length));
  }

  /** The element has a true peak to show: an over lamp, a hold marker, a readout. */
  hasTruePeak(group: MeterGroup, id: string): boolean {
    const slot = this.slots.get(slotKey(group, id));
    return slot !== undefined && this.showsTruePeak(slotKey(group, id), slot);
  }

  ingest(replica: string, values: Float32Array, now: number): void {
    this.replica = replica;
    this.receivedAt = now;
    this.values = values;
    this.readings.clear();
    this.history.push({ at: now, values });
    if (this.history.length > HISTORY) this.history.shift();
    // Each replica measures loudness itself: a reset shows only between
    // frames of one replica.
    if (replica !== this.loudReplica) {
      this.loudReplica = replica;
      this.loudSeen.clear();
    }
    for (const [key, slot] of this.truePeaks) {
      if (slot.kind === 'bus' && this.restarted(key, slot, values)) {
        this.overs.delete(key);
        this.tpMax.delete(key);
      }
      const tp = slot.offset + slot.legs;
      let max = values[tp]!;
      for (let i = 1; i < slot.legs; i++) max = Math.max(max, values[tp + i]!);
      if (max > (this.tpMax.get(key) ?? -Infinity)) this.tpMax.set(key, max);
      if (max > this.truePeakMax && !this.overs.has(key)) this.overs.add(key);
    }
  }

  /**
   * The element's loudness measurement restarted since the last frame:
   * a RESET from any client. The maxima since reset only rise until one;
   * integrated and LRA are null only until enough audio is measured after
   * one; LRA is unstable for 60 s after one (model.schema.json Loudness).
   * A reset at the moment the maxima equal the current level shows only
   * in integrated and LRA.
   */
  private restarted(key: string, slot: MeterSlot, values: Float32Array): boolean {
    const t = slot.offset + 3 * slot.legs;
    const now: LoudSeen = {
      integrated: values[t + 2]!,
      lra: values[t + 3]!,
      maxMomentary: values[t + 4]!,
      maxShortTerm: values[t + 5]!,
      lraStable: (values[t + 7]! & 2) === 2,
    };
    const prev = this.loudSeen.get(key);
    this.loudSeen.set(key, now);
    if (!prev) return false;
    const fell = (a: number, b: number) => !Number.isNaN(a) && (Number.isNaN(b) || b < a);
    const cleared = (a: number, b: number) => !Number.isNaN(a) && Number.isNaN(b);
    return (
      fell(prev.maxMomentary, now.maxMomentary) ||
      fell(prev.maxShortTerm, now.maxShortTerm) ||
      cleared(prev.integrated, now.integrated) ||
      cleared(prev.lra, now.lra) ||
      (prev.lraStable && !now.lraStable)
    );
  }

  /** Highest true peak of the element since its over was last cleared, dBTP, or null. */
  truePeakSinceReset(group: MeterGroup, id: string): number | null {
    return this.tpMax.get(slotKey(group, id)) ?? null;
  }

  /** Clears the latched true-peak overs and true-peak maxima of these elements. */
  clearOvers(group: MeterGroup, ids: readonly string[]): void {
    for (const id of ids) {
      this.overs.delete(slotKey(group, id));
      this.tpMax.delete(slotKey(group, id));
    }
  }

  /**
   * Highest value per leg of `kind` over every frame that arrived after
   * `since` (ms, the clock of ingest), into `out`; false when none did.
   * A display that draws less often than frames arrive misses no peak.
   */
  maxSince(group: MeterGroup, id: string, kind: 'peak' | 'truePeak', since: number, out: number[]): boolean {
    const slot = this.slots.get(slotKey(group, id));
    if (!slot || (kind === 'truePeak' && !this.showsTruePeak(slotKey(group, id), slot))) return false;
    const base = slot.offset + (kind === 'truePeak' ? slot.legs : 0);
    let any = false;
    for (let f = this.history.length - 1; f >= 0; f--) {
      const frame = this.history[f]!;
      if (frame.at <= since) break;
      for (let i = 0; i < slot.legs; i++) {
        const v = frame.values[base + i]!;
        if (!any || v > out[i]!) out[i] = v;
      }
      any = true;
    }
    return any;
  }

  get(group: MeterGroup, id: string): MeterReading | undefined {
    const key = slotKey(group, id);
    const cached = this.readings.get(key);
    if (cached) return cached;
    const v = this.values;
    const slot = this.slots.get(key);
    if (!v || !slot) return undefined;
    const legs = (i: number) => Array.from(v.subarray(slot.offset + i * slot.legs, slot.offset + (i + 1) * slot.legs));
    const opt = (x: number) => (Number.isNaN(x) ? null : x);
    let reading: MeterReading;
    if (slot.kind === 'monitor') {
      reading = { peak: legs(0), truePeak: [] };
    } else if (slot.kind === 'channel') {
      reading = { peak: legs(0), truePeak: legs(1), momentary: opt(v[slot.offset + 2 * slot.legs]!) };
    } else {
      const t = slot.offset + 3 * slot.legs;
      const flags = v[t + 7]!;
      reading = {
        peak: legs(0),
        truePeak: legs(1),
        rms: legs(2),
        loudness: {
          momentary: opt(v[t]!),
          shortTerm: opt(v[t + 1]!),
          integrated: opt(v[t + 2]!),
          lra: opt(v[t + 3]!),
          maxMomentary: opt(v[t + 4]!),
          maxShortTerm: opt(v[t + 5]!),
          integratedValid: (flags & 1) === 1,
          lraStable: (flags & 2) === 2,
        },
        correlation: opt(v[t + 6]!),
      };
    }
    this.readings.set(key, reading);
    return reading;
  }

  /** Forget the latest frames, so bars fall. Latched overs stay. */
  clear(): void {
    this.values = null;
    this.readings.clear();
    this.history = [];
  }
}

type Draw = (now: number, dt: number) => void;

const draws = new Set<Draw>();
let frame = 0;
let last = 0;

function tick(now: number): void {
  const dt = last === 0 ? 16 : Math.min(now - last, 200);
  last = now;
  for (const d of draws) d(now, dt);
  frame = draws.size > 0 ? requestAnimationFrame(tick) : 0;
}

/** Register a callback on the shared animation-frame loop. */
export function onAnimationFrame(draw: Draw): () => void {
  draws.add(draw);
  if (frame === 0 && typeof requestAnimationFrame === 'function') {
    last = 0;
    frame = requestAnimationFrame(tick);
  }
  return () => {
    draws.delete(draw);
  };
}
