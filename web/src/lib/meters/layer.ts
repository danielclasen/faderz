// One canvas that draws every meter of a view. Each meter is an element
// in the page that only reserves the space and carries the meter's
// accessible name; the layer draws into those rectangles. A single
// canvas is one composited layer, redrawn only on a frame with a new
// meter reading, a layout change, or bars still falling after the
// readings went stale. One canvas per meter was one layer each, all
// redrawn on every animation frame.

import { getContext, setContext } from 'svelte';
import { METER_FLOOR_DB, METER_MARKS, meterPosition, newLeg, stepLeg, type LegBallistics } from './ballistics';
import { onAnimationFrame, type MeterGroup, type MeterStore } from './store';

export const STALE_MS = 500;
const MID = -18;
const HIGH = -6;

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
  el: HTMLElement;
  /** Nearest ancestor below the host that clips its content, or null; undefined until measured. */
  clipper: HTMLElement | null | undefined;
  peak: LegBallistics[];
  rms: LegBallistics[];
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
  private colors: Colors = { low: '', mid: '', high: '', well: '', line: '', text: '' };

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

  register(el: HTMLElement, spec: MeterSpec): () => void {
    const legs = (n: number) => Array.from({ length: n }, newLeg);
    this.slots.set(el, { spec, el, clipper: undefined, peak: legs(spec.legs), rms: legs(spec.legs), rect: { x: 0, y: 0, w: 0, h: 0 }, clip: null, visible: false });
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
    if (!arrived && !relayout && (fresh || !this.falling())) return;
    this.lastReceived = received;
    const dt = this.drawn ? now - this.lastDraw : 0;
    this.lastDraw = now;
    this.drawn = true;
    for (const s of this.slots.values()) this.step(s, fresh, now, dt);
    this.draw();
  }

  /** Some bar or peak hold is still above the floor. */
  private falling(): boolean {
    for (const s of this.slots.values()) {
      for (const l of s.peak) if (l.bar > METER_FLOOR_DB || l.hold > METER_FLOOR_DB) return true;
      for (const l of s.rms) if (l.bar > METER_FLOOR_DB) return true;
    }
    return false;
  }

  private step(s: Slot, fresh: boolean, now: number, dt: number): void {
    const reading = fresh ? this.meters.get(s.spec.group, s.spec.id) : undefined;
    for (let i = 0; i < s.spec.legs; i++) {
      stepLeg(s.peak[i]!, reading?.peak[i] ?? METER_FLOOR_DB, now, dt);
      stepLeg(s.rms[i]!, reading?.rms[i] ?? METER_FLOOR_DB, now, dt);
    }
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
    const { x: x0, y: y0, w, h } = s.rect;
    const { legs, scale } = s.spec;
    const pos = (db: number) => y0 + h - meterPosition(db) * h;
    const bar = (x: number, bw: number, fromDb: number, toDb: number, color: string, alpha: number) => {
      const yFrom = pos(fromDb);
      const yTo = pos(toDb);
      if (yFrom - yTo <= 0) return;
      ctx.globalAlpha = alpha;
      ctx.fillStyle = color;
      ctx.fillRect(x, yTo, bw, yFrom - yTo);
    };
    const zoned = (x: number, bw: number, value: number, alpha: number) => {
      bar(x, bw, METER_FLOOR_DB, Math.min(value, MID), colors.low, alpha);
      if (value > MID) bar(x, bw, MID, Math.min(value, HIGH), colors.mid, alpha);
      if (value > HIGH) bar(x, bw, HIGH, value, colors.high, alpha);
    };
    const zoneColor = (v: number) => (v > HIGH ? colors.high : v > MID ? colors.mid : colors.low);

    const scaleW = scale ? 18 : 0;
    ctx.globalAlpha = 1;
    ctx.fillStyle = colors.well;
    ctx.fillRect(x0, y0, w, h);
    const inner = w - scaleW;
    const gap = legs > 1 ? 1 : 0;
    const bw = Math.max(1, (inner - gap * (legs - 1)) / legs);
    for (let i = 0; i < legs; i++) {
      const x = x0 + scaleW + i * (bw + gap);
      zoned(x, bw, s.peak[i]!.bar, 0.45);
      zoned(x, bw, s.rms[i]!.bar, 1);
      const hold = s.peak[i]!.hold;
      if (hold > METER_FLOOR_DB) {
        ctx.globalAlpha = 1;
        ctx.fillStyle = zoneColor(hold);
        ctx.fillRect(x, pos(hold) - 1, bw, 2);
      }
    }
    ctx.globalAlpha = 1;
    ctx.fillStyle = colors.line;
    for (const mark of METER_MARKS) {
      const y = Math.round(pos(mark)) + 0.5;
      ctx.fillRect(x0 + scaleW, y, inner, mark === MID ? 1 : 0.5);
    }
    if (scale) {
      ctx.fillStyle = colors.text;
      ctx.font = '8px "JetBrains Mono", monospace';
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';
      for (const mark of METER_MARKS) {
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
