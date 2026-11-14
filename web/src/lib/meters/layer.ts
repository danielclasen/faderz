// One canvas that draws every meter of a view. Each meter is an element
// in the page that only reserves the space and carries the meter's
// accessible name; the layer draws into those rectangles. A single
// canvas is one composited layer, redrawn only on a frame with a new
// meter reading, a layout change, or bars still falling after the
// readings went stale. One canvas per meter was one layer each, all
// redrawn on every animation frame.

import { getContext, setContext } from 'svelte';
import { METER_FLOOR_DB, meterMarks, meterPosition, newLeg, stepLeg, type LegBallistics } from './ballistics';
import { onAnimationFrame, type MeterGroup, type MeterStore } from './store';
import { slotKey } from './flat';

export const STALE_MS = 500;
/** Meter colour boundaries, dBFS (State.metering.zones). */
export interface MeterZones {
  alignment: number;
  permittedMax: number;
  /** True peak above which a meter latches its over lamp, dBTP. */
  truePeakMax: number;
}
/** Width of the momentary loudness bar of a channel meter, px. */
const LOUD_W = 3;
/** Height of the true-peak over lamp at the top of every meter, px, and the gap below it. */
const LAMP_H = 3;
const LAMP_GAP = 1;

export interface MeterSpec {
  group: MeterGroup;
  id: string;
  legs: number;
  scale: boolean;
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Slot {
  spec: MeterSpec;
  /** Slot key in the meter store, group/id. */
  key: string;
  el: HTMLElement;
  /** Nearest ancestor below the host that clips its content, or null; undefined until measured. */
  clipper: HTMLElement | null | undefined;
  peak: LegBallistics[];
  /** True peak: of the bus, or of a channel at its direct-out tap. Drawn as its hold marker. */
  tp: LegBallistics[];
  /** Bus RMS legs; empty for channels. */
  rms: LegBallistics[];
  /** Channel momentary loudness at the meter tap; null for buses. */
  loud: LegBallistics | null;
  rect: Rect;
  clip: Rect | null;
  /** In the viewport, by an IntersectionObserver: only these are measured and drawn. */
  visible: boolean;
}

interface Colors {
  low: string;
  mid: string;
  high: string;
  well: string;
  line: string;
  text: string;
  loud: string;
  tp: string;
}

export class MeterLayer {
  private readonly slots = new Map<HTMLElement, Slot>();
  private host: HTMLElement | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;
  private readonly ro = new ResizeObserver(() => this.relayout());
  private readonly mo = new MutationObserver(() => {
    this.readColors();
    this.relayout();
  });
  private stop: (() => void) | null = null;
  /** Reading the box of a meter in a skipped (content-visibility) strip would force its layout. */
  private readonly io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      const s = this.slots.get(e.target as HTMLElement);
      if (s) s.visible = e.isIntersecting;
    }
    this.relayout();
  });
  private layoutDirty = true;
  private drawn = false;
  private lastReceived = -1;
  private lastDraw = 0;
  private dpr = 1;
  private w = 0;
  private h = 0;
  private zones: MeterZones = { alignment: -18, permittedMax: -9, truePeakMax: -1 };
  /** Scratch legs for the maxima of the frames since the last draw. */
  private readonly peakIn: number[] = [];
  private readonly tpIn: number[] = [];
  /** Latched overs drawn last, to redraw when one latches or clears. */
  private oversDrawn = 0;
  private marks: number[] = meterMarks(this.zones);
  private colors: Colors = { low: '', mid: '', high: '', well: '', line: '', text: '', loud: '', tp: '' };

  constructor(private readonly meters: MeterStore) {}

  /** Starts drawing into `canvas`, which covers `host`; meters register before or after. */
  attach(host: HTMLElement, canvas: HTMLCanvasElement): void {
    this.host = host;
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d')!;
    this.readColors();
    this.ro.observe(host);
    this.mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    host.addEventListener('scroll', this.relayout, { capture: true, passive: true });
    window.addEventListener('resize', this.relayout);
    this.stop = onAnimationFrame((now) => this.frame(now));
  }

  /** Colour boundaries and marked levels follow the shared metering settings. */
  setZones(zones: MeterZones): void {
    if (zones.alignment === this.zones.alignment && zones.permittedMax === this.zones.permittedMax && zones.truePeakMax === this.zones.truePeakMax) return;
    this.zones = { alignment: zones.alignment, permittedMax: zones.permittedMax, truePeakMax: zones.truePeakMax };
    this.marks = meterMarks(this.zones);
    this.relayout();
  }

  register(el: HTMLElement, spec: MeterSpec): () => void {
    const legs = (n: number) => Array.from({ length: n }, newLeg);
    const channel = spec.group === 'channels';
    this.slots.set(el, {
      spec,
      key: slotKey(spec.group, spec.id),
      el,
      clipper: undefined,
      peak: legs(spec.legs),
      tp: legs(spec.legs),
      rms: channel ? [] : legs(spec.legs),
      loud: channel ? newLeg() : null,
      rect: { x: 0, y: 0, w: 0, h: 0 },
      clip: null,
      visible: false,
    });
    this.ro.observe(el);
    this.io.observe(el);
    this.relayout();
    return () => {
      this.slots.delete(el);
      this.ro.unobserve(el);
      this.io.unobserve(el);
      this.relayout();
    };
  }

  destroy(): void {
    this.stop?.();
    this.ro.disconnect();
    this.io.disconnect();
    this.mo.disconnect();
    this.host?.removeEventListener('scroll', this.relayout, { capture: true });
    window.removeEventListener('resize', this.relayout);
  }

  private clipperOf(el: HTMLElement): HTMLElement | null {
    for (let p = el.parentElement; p && p !== this.host; p = p.parentElement) {
      const o = getComputedStyle(p);
      if (o.overflowX !== 'visible' || o.overflowY !== 'visible') return p;
    }
    return null;
  }

  private readonly relayout = (): void => {
    this.layoutDirty = true;
  };

  private readColors(): void {
    const style = getComputedStyle(document.documentElement);
    const v = (name: string) => style.getPropertyValue(name).trim();
    this.colors = {
      low: v('--meter-low'),
      mid: v('--meter-mid'),
      high: v('--meter-high'),
      well: v('--well'),
      line: v('--line-strong'),
      text: v('--text-faint'),
      loud: v('--meter-loud'),
      tp: v('--meter-tp'),
    };
  }

  /** Reads every rectangle once, relative to the canvas, after a layout change. */
  private measure(): void {
    this.layoutDirty = false;
    const canvas = this.canvas!;
    const c = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    if (c.width !== this.w || c.height !== this.h || dpr !== this.dpr) {
      this.w = c.width;
      this.h = c.height;
      this.dpr = dpr;
      canvas.width = Math.max(1, Math.round(c.width * dpr));
      canvas.height = Math.max(1, Math.round(c.height * dpr));
    }
    const rel = (r: DOMRect): Rect => ({ x: r.left - c.left, y: r.top - c.top, w: r.width, h: r.height });
    const clips = new Map<HTMLElement, Rect>();
    for (const s of this.slots.values()) {
      if (!s.visible) continue;
      s.rect = rel(s.el.getBoundingClientRect());
      if (s.clipper === undefined) s.clipper = this.clipperOf(s.el);
      if (!s.clipper) {
        s.clip = null;
        continue;
      }
      let clip = clips.get(s.clipper);
      if (!clip) {
        clip = rel(s.clipper.getBoundingClientRect());
        clips.set(s.clipper, clip);
      }
      s.clip = clip;
    }
  }

  private frame(now: number): void {
    const relayout = this.layoutDirty;
    if (relayout) this.measure();
    if (this.w === 0 || this.h === 0) return;
    const received = this.meters.receivedAt;
    const fresh = received > 0 && now - received < STALE_MS;
    const arrived = received !== this.lastReceived;
    const overs = this.meters.overs.size;
    if (!arrived && !relayout && overs === this.oversDrawn && (fresh || !this.falling())) return;
    // Every frame that arrived since the last draw counts, not only the latest.
    const since = arrived ? this.lastReceived : received - 1e-3;
    this.lastReceived = received;
    this.oversDrawn = overs;
    const dt = this.drawn ? now - this.lastDraw : 0;
    this.lastDraw = now;
    this.drawn = true;
    for (const s of this.slots.values()) this.step(s, fresh, since, now, dt);
    this.draw();
  }

  /** Some bar or peak hold is still above the floor. */
  private falling(): boolean {
    for (const s of this.slots.values()) {
      for (const l of s.peak) if (l.bar > METER_FLOOR_DB || l.hold > METER_FLOOR_DB) return true;
      for (const l of s.tp) if (l.hold > METER_FLOOR_DB) return true;
      for (const l of s.rms) if (l.bar > METER_FLOOR_DB) return true;
      if (s.loud && s.loud.bar > METER_FLOOR_DB) return true;
    }
    return false;
  }

  private step(s: Slot, fresh: boolean, since: number, now: number, dt: number): void {
    const { group, id } = s.spec;
    const reading = fresh ? this.meters.get(group, id) : undefined;
    const peak = fresh && this.meters.maxSince(group, id, 'peak', since, this.peakIn);
    const tp = fresh && this.meters.maxSince(group, id, 'truePeak', since, this.tpIn);
    for (let i = 0; i < s.spec.legs; i++) {
      stepLeg(s.peak[i]!, peak ? this.peakIn[i]! : METER_FLOOR_DB, now, dt);
      stepLeg(s.tp[i]!, tp ? this.tpIn[i]! : METER_FLOOR_DB, now, dt);
      if (s.rms[i]) stepLeg(s.rms[i]!, reading?.rms?.[i] ?? METER_FLOOR_DB, now, dt);
    }
    if (s.loud) stepLeg(s.loud, reading?.momentary ?? METER_FLOOR_DB, now, dt);
  }

  private draw(): void {
    const ctx = this.ctx!;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.w, this.h);
    for (const s of this.slots.values()) {
      if (!s.visible) continue;
      const r = s.rect;
      const c = s.clip ?? { x: 0, y: 0, w: this.w, h: this.h };
      const left = Math.max(r.x, c.x);
      const right = Math.min(r.x + r.w, c.x + c.w);
      const top = Math.max(r.y, c.y);
      const bottom = Math.min(r.y + r.h, c.y + c.h);
      if (right <= left || bottom <= top) continue;
      const partial = left > r.x || right < r.x + r.w || top > r.y || bottom < r.y + r.h;
      if (partial) {
        ctx.save();
        ctx.beginPath();
        ctx.rect(left, top, right - left, bottom - top);
        ctx.clip();
      }
      this.drawMeter(s);
      if (partial) ctx.restore();
    }
  }

  private drawMeter(s: Slot): void {
    const ctx = this.ctx!;
    const colors = this.colors;
    const { x: x0, y: top, w, h: full } = s.rect;
    const { legs, scale } = s.spec;
    // The over lamp takes the top of the box; the scale runs below it.
    const y0 = top + LAMP_H + LAMP_GAP;
    const h = full - LAMP_H - LAMP_GAP;
    const pos = (db: number) => y0 + h - meterPosition(db) * h;
    const bar = (x: number, bw: number, fromDb: number, toDb: number, color: string, alpha: number) => {
      const yFrom = pos(fromDb);
      const yTo = pos(toDb);
      if (yFrom - yTo <= 0) return;
      ctx.globalAlpha = alpha;
      ctx.fillStyle = color;
      ctx.fillRect(x, yTo, bw, yFrom - yTo);
    };
    const MID = this.zones.alignment;
    const HIGH = this.zones.permittedMax;
    const zoned = (x: number, bw: number, value: number, alpha: number) => {
      bar(x, bw, METER_FLOOR_DB, Math.min(value, MID), colors.low, alpha);
      if (value > MID) bar(x, bw, MID, Math.min(value, HIGH), colors.mid, alpha);
      if (value > HIGH) bar(x, bw, HIGH, value, colors.high, alpha);
    };
    const zoneColor = (v: number) => (v > HIGH ? colors.high : v > MID ? colors.mid : colors.low);

    const scaleW = scale ? 18 : 0;
    ctx.globalAlpha = 1;
    ctx.fillStyle = colors.well;
    ctx.fillRect(x0, top, w, full);
    const over = this.meters.overs.has(s.key);
    ctx.globalAlpha = over ? 1 : 0.5;
    ctx.fillStyle = over ? colors.high : colors.line;
    ctx.fillRect(x0 + (scale ? 18 : 0), top, w - (scale ? 18 : 0), LAMP_H);
    ctx.globalAlpha = 1;
    // A channel meter gives its rightmost LOUD_W pixels to one momentary
    // loudness bar; the legs share the rest.
    const loudW = s.loud ? LOUD_W : 0;
    const inner = w - scaleW;
    const legsW = inner - (loudW ? loudW + 1 : 0);
    const gap = legs > 1 ? 1 : 0;
    const bw = Math.max(1, (legsW - gap * (legs - 1)) / legs);
    if (s.loud && s.loud.bar > METER_FLOOR_DB) {
      bar(x0 + w - loudW, loudW, METER_FLOOR_DB, s.loud.bar, colors.loud, 1);
    }
    for (let i = 0; i < legs; i++) {
      const x = x0 + scaleW + i * (bw + gap);
      if (s.rms[i]) {
        zoned(x, bw, s.peak[i]!.bar, 0.45);
        zoned(x, bw, s.rms[i]!.bar, 1);
      } else {
        zoned(x, bw, s.peak[i]!.bar, 1);
      }
      const hold = s.peak[i]!.hold;
      if (hold > METER_FLOOR_DB) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = zoneColor(hold);
        ctx.fillRect(x, pos(hold) - 1, bw, 2);
      }
      const tp = s.tp[i]!.hold;
      if (tp > METER_FLOOR_DB) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = tp > this.zones.truePeakMax ? colors.high : colors.tp;
        ctx.fillRect(x, Math.round(pos(Math.min(tp, 0))) - 0.5, bw, 1);
      }
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = colors.line;
    for (const mark of this.marks) {
      const y = Math.round(pos(mark)) + 0.5;
      ctx.fillRect(x0 + scaleW, y, inner, mark === MID ? 2 : 0.5);
    }
    if (scale) {
      ctx.fillStyle = colors.text;
      ctx.font = '8px "JetBrains Mono", monospace';
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';
      for (const mark of this.marks) {
        const y = Math.min(y0 + h - 5, Math.max(y0 + 5, pos(mark)));
        ctx.fillText(String(mark), x0 + scaleW - 3, y);
      }
    }
  }
}

const KEY = Symbol('meter-layer');

/** Makes `layer` the one that draws every Meter below the calling component. */
export function provideMeterLayer(layer: MeterLayer): MeterLayer {
  return setContext(KEY, layer);
}

export function meterLayer(): MeterLayer {
  const layer = getContext<MeterLayer | undefined>(KEY);
  if (!layer) throw new Error('Meter outside a meter layer');
  return layer;
}
