// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import TalkKey from '../../src/components/TalkKey.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const ACTIVE = '/mixMinus/mm1/interrupt/active';
const SOURCE = '/mixMinus/mm1/interrupt/source';
const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

/** TALK key of mm1 with a talkback source, latched by a short Space press. */
function latched() {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [[SOURCE, 'ch1']], owners: [], status: LIVE });
  const target = document.createElement('div');
  document.body.append(target);
  const key = mount(TalkKey, { target, props: { id: 'mm1', name: 'N-1 Phone 1' }, context: mixerContext(s) });
  flushSync();
  const button = target.querySelector('button')!;
  button.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
  button.dispatchEvent(new KeyboardEvent('keyup', { key: ' ', bubbles: true }));
  flushSync();
  expect(button.getAttribute('aria-label')).toBe('N-1 Phone 1 talk, latched');
  posted.length = 0;
  return { s, posted, shape, key, button };
}

describe('TALK key', () => {
  it('ends a latched talk when it unmounts: clears and releases it', async () => {
    const { posted, key } = latched();
    unmount(key);
    flushSync();
    await Promise.resolve();
    expect(posted).toEqual([
      { type: 'set', path: ACTIVE, value: false },
      { type: 'release', path: ACTIVE },
    ]);
  });

  it('only releases a latched talk whose mix-minus a layout change removed', async () => {
    // The new shape has no such path: a set would create it locally and
    // the server would refuse it with an error notice.
    const { s, posted, shape, key } = latched();
    const without = { ...shape, mixMinus: shape.mixMinus.filter((m) => m.id !== 'mm1') };
    s.apply({ type: 'patch', input: 9, snapshot: { shape: without, state: initialState(without), owners: {} }, changes: [], owners: [], status: LIVE });
    unmount(key);
    flushSync();
    await Promise.resolve();
    expect(posted).toEqual([{ type: 'release', path: ACTIVE }]);
    expect(s.state!.mixMinus['mm1']).toBeUndefined();
  });

  it('ends a latched talk when the page is hidden or left', () => {
    // A mouse hold over a tab switch gets no blur on Safari, which does
    // not focus a button on click.
    for (const leave of [
      () => window.dispatchEvent(new Event('pagehide')),
      () => {
        Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
        document.dispatchEvent(new Event('visibilitychange'));
        delete (document as { visibilityState?: unknown }).visibilityState;
      },
    ]) {
      const { posted, key, button } = latched();
      leave();
      flushSync();
      expect(posted).toEqual([
        { type: 'set', path: ACTIVE, value: false },
        { type: 'release', path: ACTIVE },
      ]);
      expect(button.getAttribute('aria-label')).toBe('N-1 Phone 1 talk');
      unmount(key);
      document.body.replaceChildren();
    }
  });

  it('ends a held talk when the page is hidden, and leaves an idle key alone', () => {
    const { posted, key, button } = latched();
    button.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
    button.dispatchEvent(new KeyboardEvent('keyup', { key: ' ', bubbles: true }));
    flushSync();
    posted.length = 0;
    button.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(posted.map((m) => m.type)).toEqual(['touch', 'set']);
    window.dispatchEvent(new Event('pagehide'));
    flushSync();
    expect(posted.slice(2)).toEqual([
      { type: 'set', path: ACTIVE, value: false },
      { type: 'release', path: ACTIVE },
    ]);
    posted.length = 0;
    window.dispatchEvent(new Event('pagehide'));
    expect(posted).toEqual([]);
    unmount(key);
    document.body.replaceChildren();
  });
});
