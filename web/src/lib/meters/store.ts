// Latest meter frame, as the flattened values the sync worker transfers
// (meters/flat.ts). Kept outside Svelte reactivity: canvases read it from
// one animation-frame loop instead of re-rendering at 25 Hz.

import { deriveMeterLayout } from '../contract/meterFrame';
import type { BusMeter, Shape } from '../contract/types';
import { meterSlots, slotKey, type MeterGroup, type MeterSlot } from './flat';

export type { MeterGroup } from './flat';

export interface MeterReading {
  peak: number[];
  truePeak: number[];
  /** Bus, aux and mix-minus only. */
  rms?: number[];
  /** Channels: momentary loudness at the meter tap. */
  momentary?: number | null;
  loudness?: BusMeter['loudness'];
  correlation?: number | null;
}

export class MeterStore {
  private slots = new Map<string, MeterSlot>();
  private values: Float32Array | null = null;
  private readings = new Map<string, MeterReading>();
  /** Replica that measured the latest frame. */
  replica: string | null = null;
  receivedAt = 0;

  /** Meter frames follow the shape of the last snapshot. */
  setShape(shape: Shape): void {
    this.slots = meterSlots(deriveMeterLayout(shape));
    this.clear();
  }

  ingest(replica: string, values: Float32Array, now: number): void {
    this.replica = replica;
    this.receivedAt = now;
    this.values = values;
    this.readings.clear();
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
    if (!slot.bus) {
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

  /** Forget the latest frame, so bars fall. */
  clear(): void {
    this.values = null;
    this.readings.clear();
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
