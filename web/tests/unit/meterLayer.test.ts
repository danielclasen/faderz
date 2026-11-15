// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MeterLayer } from '../../src/lib/meters/layer';
import { MeterStore } from '../../src/lib/meters/store';

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The page box of `el`; a canvas adds its translate, as layout does. */
function place(el: HTMLElement, box: Box): void {
  el.getBoundingClientRect = () => {
    const t = /translate\((-?[\d.]+)px, (-?[\d.]+)px\)/.exec(el.style.transform);
    const x = box.x + (t ? Number(t[1]) : 0);
    const y = box.y + (t ? Number(t[2]) : 0);
    return { x, y, left: x, top: y, width: box.w, height: box.h, right: x + box.w, bottom: y + box.h, toJSON: () => ({}) } as DOMRect;
  };
}

let frames: FrameRequestCallback[] = [];
const layers: MeterLayer[] = [];
let visibility: ((entries: Array<{ target: Element; isIntersecting: boolean }>) => void) | null = null;
const fills: Box[] = [];
/** Clip rectangles of the meters a scroller cuts. */
const clips: Box[] = [];

beforeEach(() => {
  fills.length = 0;
  clips.length = 0;
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => frames.push(cb));
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(cb: typeof visibility) {
        visibility = cb;
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});
afterEach(() => {
  // The animation-frame loop is shared: it stops once no layer is left.
  for (const l of layers.splice(0)) l.destroy();
  frame(0);
  vi.unstubAllGlobals();
});

function frame(now: number): void {
  const due = frames;
  frames = [];
  for (const cb of due) cb(now);
}

function context(): CanvasRenderingContext2D {
  const ctx: Record<string, unknown> = {
    fillRect: (x: number, y: number, w: number, h: number) => fills.push({ x, y, w, h }),
    fillText() {},
    clearRect() {},
    setTransform() {},
    save() {},
    restore() {},
    beginPath() {},
    rect: (x: number, y: number, w: number, h: number) => clips.push({ x, y, w, h }),
    clip() {},
  };
  return ctx as unknown as CanvasRenderingContext2D;
}

/** A 1600x932 view with a meter layer over it, as the mixer has. */
function view() {
  const host = document.createElement('div');
  const canvas = document.createElement('canvas');
  host.append(canvas);
  document.body.append(host);
  place(host, { x: 0, y: 44, w: 1600, h: 932 });
  place(canvas, { x: 0, y: 44, w: 1600, h: 932 });
  canvas.getContext = (() => context()) as unknown as HTMLCanvasElement['getContext'];
  const layer = new MeterLayer(new MeterStore());
  layer.attach(host, canvas);
  layers.push(layer);
  const meter = (parent: HTMLElement, box: Box, id: string) => {
    const el = document.createElement('div');
    parent.append(el);
    place(el, box);
    layer.register(el, { group: 'channels', id, legs: 1, scale: null });
    return el;
  };
  return { host, canvas, layer, meter };
}

/** Every fill lies on the canvas, whose CSS box is `w` by `h`. */
function inside(w: number, h: number): boolean {
  return fills.every((f) => f.x >= 0 && f.y >= 0 && f.x + f.w <= w && f.y + f.h <= h);
}

describe('meter layer canvas', () => {
  it('covers the visible meters only, not the whole view', () => {
    const { host, canvas, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    const b = meter(host, { x: 146, y: 320, w: 30, h: 580 }, 'ch2');
    const hidden = meter(host, { x: 1400, y: 100, w: 30, h: 580 }, 'ch3');
    visibility!([
      { target: a, isIntersecting: true },
      { target: b, isIntersecting: true },
      { target: hidden, isIntersecting: false },
    ]);
    frame(16);
    // Each frame drawn copies every pixel of the canvas: a 1600x932
    // backing store is 13x the 118x582 the two meters and the margin use.
    expect([canvas.width, canvas.height]).toEqual([118, 582]);
    expect([canvas.style.width, canvas.style.height]).toEqual(['118px', '582px']);
    expect(canvas.style.transform).toBe('translate(59px, 275px)');
    // The wells are drawn where the meters are on the page.
    expect(fills[0]).toEqual({ x: 1, y: 1, w: 30, h: 580 });
    expect(fills.some((f) => f.x === 87 && f.y === 1 && f.w === 30 && f.h === 580)).toBe(true);
  });

  it('keeps the floor mark, on the row below a meter, on the canvas', () => {
    const { host, canvas, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    // The floor mark: a hairline at the meter's bottom edge plus half a pixel.
    expect(fills.some((f) => f.y === 581.5 && f.h === 0.5)).toBe(true);
    expect(inside(32, 582)).toBe(true);
    expect([canvas.style.width, canvas.style.height]).toEqual(['32px', '582px']);
  });

  it('follows the meters after a layout change, measured from its place before the transform', () => {
    const { host, canvas, layer, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    expect(canvas.style.transform).toBe('translate(59px, 275px)');
    place(a, { x: 200, y: 300, w: 30, h: 600 });
    (layer as unknown as { relayout: () => void }).relayout();
    fills.length = 0;
    frame(32);
    expect([canvas.width, canvas.height]).toEqual([32, 602]);
    expect(canvas.style.transform).toBe('translate(199px, 255px)');
    expect(fills[0]).toEqual({ x: 1, y: 1, w: 30, h: 600 });
  });

  it('leaves out the part of a meter its scroller clips, and clips that meter in canvas coordinates', () => {
    const { host, canvas, meter } = view();
    const scroller = document.createElement('div');
    scroller.style.overflowX = 'auto';
    host.append(scroller);
    place(scroller, { x: 0, y: 75, w: 822, h: 900 });
    const a = meter(scroller, { x: 740, y: 320, w: 30, h: 580 }, 'ch9');
    const cut = meter(scroller, { x: 812, y: 320, w: 30, h: 580 }, 'ch10');
    visibility!([
      { target: a, isIntersecting: true },
      { target: cut, isIntersecting: true },
    ]);
    frame(16);
    expect([canvas.width, canvas.height]).toEqual([84, 582]);
    expect(canvas.style.transform).toBe('translate(739px, 275px)');
    // The scroller's right edge, x 822 on the page, is x 83 on the canvas:
    // only the 10 px of the cut meter left of it are drawn.
    expect(clips).toEqual([{ x: 73, y: 1, w: 10, h: 580 }]);
  });

  it('gives the backing store a device pixel per device pixel at devicePixelRatio 2', () => {
    vi.stubGlobal('devicePixelRatio', 2);
    const { host, canvas, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    expect([canvas.style.width, canvas.style.height]).toEqual(['32px', '582px']);
    expect([canvas.width, canvas.height]).toEqual([64, 1164]);
  });

  it('puts the canvas edges on whole device pixels at devicePixelRatio 1.25', () => {
    vi.stubGlobal('devicePixelRatio', 1.25);
    const { host, canvas, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    // At 59 CSS px a canvas edge would sit at device pixel 73.75 and the
    // backing store would be resampled; it starts at device pixel 73.
    expect(canvas.style.transform).toBe('translate(58.4px, 274.4px)');
    expect([canvas.style.width, canvas.style.height]).toEqual(['32.8px', '583.2px']);
    expect([canvas.width, canvas.height]).toEqual([41, 729]);
    expect(inside(32.8, 583.2)).toBe(true);
  });

  it('shrinks to nothing, and draws nothing, once no meter is visible', () => {
    const { host, canvas, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    visibility!([{ target: a, isIntersecting: false }]);
    fills.length = 0;
    frame(32);
    expect([canvas.width, canvas.height]).toEqual([1, 1]);
    expect([canvas.style.width, canvas.style.height]).toEqual(['0px', '0px']);
    expect(fills).toEqual([]);
  });
});
