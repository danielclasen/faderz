// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Param from '../../src/components/Param.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

interface Case {
  what: string;
  path: string;
  start: number;
  props: { min: number; max: number; step?: number; log?: boolean };
  fine?: 'shift' | 'latch';
}

/** Parameters whose key step was below their resolution at the low end. */
const CASES: Case[] = [
  { what: 'comp ratio from 1:1', path: '/channels/ch1/comp/ratio', start: 1, props: { min: 1, max: 20, log: true } },
  { what: 'EQ gain with Shift', path: '/channels/ch1/eq/bands/0/gain', start: 0, props: { min: -18, max: 18 }, fine: 'shift' },
  { what: 'EQ Q at 0.7', path: '/channels/ch1/eq/bands/0/q', start: 0.7, props: { min: 0.1, max: 16, step: 0.1, log: true } },
  { what: 'comp release at 5 ms', path: '/channels/ch1/comp/release', start: 5, props: { min: 5, max: 5000, step: 1, log: true } },
  { what: 'gate attack at 0.1 ms', path: '/channels/ch1/gate/attack', start: 0.1, props: { min: 0.1, max: 200, log: true } },
  { what: 'trim with FINE', path: '/channels/ch1/trim', start: 0, props: { min: -20, max: 20 }, fine: 'latch' },
  { what: 'divergence with FINE', path: '/channels/ch1/pan/divergence', start: 0.5, props: { min: 0, max: 1, step: 0.01 }, fine: 'latch' },
];

function param(c: Case) {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [[c.path, c.start]], owners: [], status: LIVE });
  s.fine = c.fine === 'latch';
  const target = document.createElement('div');
  document.body.append(target);
  const p = mount(Param, { target, props: { path: c.path, label: c.what, ...c.props }, context: mixerContext(s) });
  flushSync();
  const slider = target.querySelector<HTMLElement>('[role="slider"]')!;
  const press = (key: string) => {
    slider.dispatchEvent(new KeyboardEvent('keydown', { key, shiftKey: c.fine === 'shift', bubbles: true }));
    flushSync();
  };
  return { s, posted, p, press };
}

describe('parameter slider keys', () => {
  for (const c of CASES) {
    it(`moves ${c.what} on every press, up and down`, () => {
      const { s, posted, p, press } = param(c);
      const values = [c.start];
      for (let i = 0; i < 3; i++) {
        press('ArrowRight');
        values.push(s.value<number>(c.path));
      }
      for (let i = 0; i < 2; i++) {
        press('ArrowLeft');
        values.push(s.value<number>(c.path));
      }
      for (let i = 1; i <= 3; i++) expect(values[i]!, `${c.what} press ${i}: ${values}`).toBeGreaterThan(values[i - 1]!);
      for (let i = 4; i <= 5; i++) expect(values[i]!, `${c.what} press ${i}: ${values}`).toBeLessThan(values[i - 1]!);
      expect(posted.filter((m) => m.type === 'set')).toHaveLength(5);
      unmount(p);
      document.body.replaceChildren();
    });
  }

  it('stops at the end of the range instead of moving past it', () => {
    const { s, posted, p, press } = param(CASES[0]!);
    press('ArrowLeft');
    expect(s.value<number>(CASES[0]!.path)).toBe(1);
    expect(posted.filter((m) => m.type === 'set')).toEqual([]);
    unmount(p);
    document.body.replaceChildren();
  });
});
