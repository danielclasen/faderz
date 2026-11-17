// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ChannelStrip from '../../src/components/ChannelStrip.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

// The strip's meter draws on a view's meter layer; none is needed here.
vi.mock('../../src/lib/meters/layer', () => ({ meterLayer: () => ({ register: () => () => {} }) }));

const TAP = '/channels/ch1/meterTap';
const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

function strip(viewer = false) {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
  s.viewer = viewer;
  const target = document.createElement('div');
  document.body.append(target);
  const channel = shape.channels.find((c) => c.id === 'ch1')!;
  const c = mount(ChannelStrip, { target, props: { channel, index: 0, selected: false, onselect: () => {} }, context: mixerContext(s) });
  flushSync();
  const key = () => target.querySelector<HTMLButtonElement>('[data-testid="tap-ch1"]')!;
  const sets = () => posted.filter((m) => m.type === 'set');
  return { s, c, key, sets, target };
}

describe('channel strip meter tap key', () => {
  it('shows the tap and switches a post-fader meter to pre-fader and back through the control path', () => {
    const { c, key, sets, target } = strip();
    expect(key().textContent).toBe('POST');
    expect(key().getAttribute('aria-label')).toBe('Pres 1 meter tap, post-fader: switch to pre-fader');
    key().click();
    flushSync();
    expect(sets()).toEqual([{ type: 'set', path: TAP, value: 'preFader' }]);
    expect(key().textContent).toBe(' PRE');
    expect(target.querySelector('[role="meter"]')!.getAttribute('aria-label')).toBe('Pres 1 meter, pre-fader');
    key().click();
    flushSync();
    expect(sets().at(-1)).toEqual({ type: 'set', path: TAP, value: 'postFader' });
    unmount(c);
  });

  it('takes an input tap, set in the processing panel, to post-fader', () => {
    const { s, c, key, sets } = strip();
    s.apply({ type: 'patch', input: 0, changes: [[TAP, 'input']], owners: [] });
    flushSync();
    expect(key().textContent).toBe('  IN');
    key().click();
    expect(sets()).toEqual([{ type: 'set', path: TAP, value: 'postFader' }]);
    unmount(c);
  });

  it('shows a viewer the tap and sends nothing when pressed', () => {
    const { c, key, sets } = strip(true);
    expect(key().textContent).toBe('POST');
    expect(key().getAttribute('aria-disabled')).toBe('true');
    key().click();
    flushSync();
    expect(sets()).toEqual([]);
    expect(key().textContent).toBe('POST');
    unmount(c);
  });
});
