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

beforeEach(() => {
  fills.length = 0;
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
    rect() {},
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
    // backing store is 13x the 116x580 the two meters use.
    expect([canvas.width, canvas.height]).toEqual([116, 580]);
    expect([canvas.style.width, canvas.style.height]).toEqual(['116px', '580px']);
    expect(canvas.style.transform).toBe('translate(60px, 276px)');
    // The wells are drawn where the meters are on the page.
    expect(fills[0]).toEqual({ x: 0, y: 0, w: 30, h: 580 });
    expect(fills.some((f) => f.x === 86 && f.y === 0 && f.w === 30 && f.h === 580)).toBe(true);
  });

  it('follows the meters after a layout change, measured from its place before the transform', () => {
    const { host, canvas, layer, meter } = view();
    const a = meter(host, { x: 60, y: 320, w: 30, h: 580 }, 'ch1');
    visibility!([{ target: a, isIntersecting: true }]);
    frame(16);
    expect(canvas.style.transform).toBe('translate(60px, 276px)');
    place(a, { x: 200, y: 300, w: 30, h: 600 });
    (layer as unknown as { relayout: () => void }).relayout();
    fills.length = 0;
    frame(32);
    expect([canvas.width, canvas.height]).toEqual([30, 600]);
    expect(canvas.style.transform).toBe('translate(200px, 256px)');
    expect(fills[0]).toEqual({ x: 0, y: 0, w: 30, h: 600 });
  });

  it('leaves out the part of a meter its scroller clips', () => {
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
    expect([canvas.width, canvas.height]).toEqual([82, 580]);
    expect(canvas.style.transform).toBe('translate(740px, 276px)');
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
