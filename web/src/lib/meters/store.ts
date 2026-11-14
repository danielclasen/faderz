// Latest meter frame per element. Kept outside Svelte reactivity: canvases
// read it from one animation-frame loop instead of re-rendering at 25 Hz.

import type { BusMeter, ChannelMeter, MetersMessage } from '../contract/types';

export type MeterGroup = 'channels' | 'buses' | 'auxes' | 'mixMinus';

export interface MeterReading {
  peak: number[];
  rms: number[];
  truePeak?: number[];
  loudness?: BusMeter['loudness'];
}

export class MeterStore {
  private latest = new Map<string, MeterReading>();
  /** Replica that produced the latest frame. */
  replica: string | null = null;
  receivedAt = 0;

  ingest(msg: MetersMessage, now: number): void {
    this.replica = msg.replica.id;
    this.receivedAt = now;
    const put = (group: MeterGroup, map: Record<string, ChannelMeter | BusMeter>) => {
      for (const [id, m] of Object.entries(map)) this.latest.set(`${group}/${id}`, m);
    };
    put('channels', msg.channels);
    put('buses', msg.buses);
    put('auxes', msg.auxes);
    put('mixMinus', msg.mixMinus);
  }

  get(group: MeterGroup, id: string): MeterReading | undefined {
    return this.latest.get(`${group}/${id}`);
  }

  /** Forget all readings, e.g. after switching replica, so stale bars fall. */
  clear(): void {
    this.latest.clear();
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

/** Register a draw callback on the shared animation-frame loop. */
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
