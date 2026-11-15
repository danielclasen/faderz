// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Loudness from '../../src/components/Loudness.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const RUNNING = '/loudness/pgm20/running';
const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

function panel() {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
  const target = document.createElement('div');
  document.body.append(target);
  const c = mount(Loudness, { target, props: { group: 'buses', id: 'pgm20', onreset: () => {} }, context: mixerContext(s) });
  flushSync();
  const key = target.querySelector<HTMLButtonElement>('[data-testid="loudness-pause-pgm20"]')!;
  key.setPointerCapture = () => {};
  return { s, posted, c, key, target };
}

describe('loudness PAUSE key', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('is unlit while running and lit, reading PAUSED, while paused, with the held readouts marked', () => {
    const { s, c, key, target } = panel();
    expect(key.textContent).toBe('PAUSE');
    expect(key.getAttribute('aria-pressed')).toBe('false');
    expect(target.querySelectorAll('.held')).toHaveLength(0);
    s.apply({ type: 'patch', input: 0, changes: [[RUNNING, false]], owners: [] });
    flushSync();
    expect(key.textContent).toBe('PAUSED');
    expect(key.getAttribute('aria-pressed')).toBe('true');
    // Integrated, LRA and the two maxima hold.
    expect(target.querySelectorAll('.held')).toHaveLength(4);
    unmount(c);
  });

  it('decides at the press: a hold begun on a running integration pauses it, even if it was paused meanwhile', () => {
    const { s, posted, c, key } = panel();
    key.dispatchEvent(new PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
    s.apply({ type: 'patch', input: 0, changes: [[RUNNING, false]], owners: [] });
    flushSync();
    vi.advanceTimersByTime(500);
    expect(posted.filter((m) => m.type === 'set')).toEqual([{ type: 'set', path: RUNNING, value: false }]);
    unmount(c);
  });

  it('runs a paused integration on with one press', () => {
    const { s, posted, c, key } = panel();
    s.apply({ type: 'patch', input: 0, changes: [[RUNNING, false]], owners: [] });
    flushSync();
    key.dispatchEvent(new PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
    expect(posted.filter((m) => m.type === 'set')).toEqual([{ type: 'set', path: RUNNING, value: true }]);
    unmount(c);
  });
});
