// Decoded meter frame as one Float32Array in layout order, so the worker
// can transfer it to the page without copying. Per channel: peak legs,
// truePeak legs, momentary (NaN for null). Per bus, aux and mix-minus
// output: peak, truePeak and rms legs, then momentary, shortTerm,
// integrated, lra, maxMomentary, maxShortTerm, correlation (NaN for null)
// and flags (bit 0 integratedValid, bit 1 lraStable). Per monitor output:
// peak legs.

import type { MeterFrame, MeterLayout } from '../contract/meterFrame';
import type { BusMeter, Loudness } from '../contract/types';

export type MeterGroup = 'channels' | 'buses' | 'auxes' | 'mixMinus' | 'monitor';

export interface MeterSlot {
  offset: number;
  legs: number;
  kind: 'channel' | 'bus' | 'monitor';
}

const GROUPS: readonly MeterGroup[] = ['channels', 'buses', 'auxes', 'mixMinus', 'monitor'];
const SIZE = { channel: (legs: number) => 2 * legs + 1, bus: (legs: number) => 3 * legs + 8, monitor: (legs: number) => legs };

export const slotKey = (group: MeterGroup, id: string) => `${group}/${id}`;

/** Offset of every element in a flattened frame of this layout. */
export function meterSlots(layout: MeterLayout): Map<string, MeterSlot> {
  const slots = new Map<string, MeterSlot>();
  let offset = 0;
  for (const group of GROUPS) {
    const kind = group === 'channels' ? 'channel' : group === 'monitor' ? 'monitor' : 'bus';
    for (const e of layout[group]) {
      slots.set(slotKey(group, e.id), { offset, legs: e.legs, kind });
      offset += SIZE[kind](e.legs);
    }
  }
  return slots;
}

export function flattenMeterFrame(layout: MeterLayout, frame: MeterFrame): Float32Array {
  const out = new Float32Array(layout.valueCount);
  let o = 0;
  const put = (values: readonly number[]) => {
    for (const v of values) out[o++] = v;
  };
  for (const c of frame.channels) {
    put(c.peak);
    put(c.truePeak);
    out[o++] = c.momentary ?? Number.NaN;
  }
  const loud = (l: Loudness, correlation: number | null) => {
    for (const v of [l.momentary, l.shortTerm, l.integrated, l.lra, l.maxMomentary, l.maxShortTerm, correlation]) {
      out[o++] = v ?? Number.NaN;
    }
    out[o++] = (l.integratedValid ? 1 : 0) | (l.lraStable ? 2 : 0);
  };
  for (const b of [...frame.buses, ...frame.auxes, ...frame.mixMinus] as BusMeter[]) {
    put(b.peak);
    put(b.truePeak);
    put(b.rms);
    loud(b.loudness, b.correlation);
  }
  for (const m of frame.monitor) put(m.peak);
  return out;
}
