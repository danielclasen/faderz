// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ChannelStrip from '../../src/components/ChannelStrip.svelte';
import Detail from '../../src/components/Detail.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

// The strip's meter draws on a view's meter layer; none is needed here.
vi.mock('../../src/lib/meters/layer', () => ({ meterLayer: () => ({ register: () => () => {} }) }));

const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

function store() {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
  const target = document.createElement('div');
  document.body.append(target);
  return { s, shape, target, sets: () => posted.filter((m) => m.type === 'set') };
}

describe('tone input', () => {
  it('shows TONE and its frequency in a tone channel strip in place of a source, and ident once on', () => {
    const { s, shape, target } = store();
    const index = shape.channels.findIndex((c) => c.id === 'lu');
    const c = mount(ChannelStrip, { target, props: { channel: shape.channels[index]!, index, selected: false, onselect: () => {} }, context: mixerContext(s) });
    flushSync();
    const tag = () => target.querySelector('[data-testid="tone-lu"]')!;
    expect(tag().textContent).toBe('TONE 1.00k, line-up tone 1000 Hz at -18.0 dBFS');
    expect(target.querySelector('.source:not(.tone)')).toBeNull();
    s.apply({ type: 'patch', input: 0, changes: [['/channels/lu/tone/frequency', 440], ['/channels/lu/tone/ident', true]], owners: [] });
    flushSync();
    expect(tag().textContent).toBe('TONE 440 ID, line-up tone 440 Hz at -18.0 dBFS, ident on');
    unmount(c);
  });

  it('shows a channel reading a flow its source and no tone', () => {
    const { s, shape, target } = store();
    const c = mount(ChannelStrip, { target, props: { channel: shape.channels[0]!, index: 0, selected: false, onselect: () => {} }, context: mixerContext(s) });
    flushSync();
    expect(target.querySelector('[data-testid^="tone-"]')).toBeNull();
    expect(target.querySelector('.source')!.textContent).toBe('Studio mic 1');
    unmount(c);
  });

  it("gives a tone channel's processing panel its frequency, level and ident, and a flow channel's none", () => {
    const { s, target, sets } = store();
    const c = mount(Detail, { target, props: { id: 'lu', onclose: () => {}, onselect: () => {} }, context: mixerContext(s) });
    flushSync();
    const tone = target.querySelector('[data-section="tone"]')!;
    const ident = [...tone.querySelectorAll('button')].find((b) => b.textContent === 'IDENT')!;
    expect(ident.getAttribute('aria-pressed')).toBe('false');
    ident.click();
    flushSync();
    expect(sets()).toEqual([{ type: 'set', path: '/channels/lu/tone/ident', value: true }]);
    expect(target.querySelector('[data-path="/channels/lu/tone/frequency"]')).not.toBeNull();
    expect(target.querySelector('[data-path="/channels/lu/tone/level"]')).not.toBeNull();
    unmount(c);

    const flow = mount(Detail, { target, props: { id: 'ch1', onclose: () => {}, onselect: () => {} }, context: mixerContext(s) });
    flushSync();
    expect(target.querySelector('[data-section="tone"]')).toBeNull();
    expect(target.querySelector('[data-path*="/tone/"]')).toBeNull();
    unmount(flow);
  });
});
