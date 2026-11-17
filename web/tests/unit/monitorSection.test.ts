// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import MonitorSection from '../../src/components/MonitorSection.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { Shape } from '../../src/lib/contract/types';
import { exampleShape, initialState } from '../../dev/mock/shape';

vi.mock('../../src/lib/meters/layer', () => ({ meterLayer: () => ({ register: () => () => {} }) }));

const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

function section(edit: (shape: Shape) => void = () => {}) {
  const s = new MixerStore();
  s.attach({ post: () => {} });
  const shape = exampleShape();
  const state = initialState(shape);
  edit(shape);
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state, owners: {} }, changes: [], owners: [], status: LIVE });
  const target = document.createElement('div');
  document.body.append(target);
  const c = mount(MonitorSection, { target, context: mixerContext(s) });
  flushSync();
  const region = target.querySelector('section')!;
  const dest = target.querySelector('[data-testid="monitor-dest"]')!;
  return { c, region, dest };
}

// The console plays no audio: the section controls a mix that leaves the
// mixer on output flows, and says which.
describe('monitor section', () => {
  it('is labelled as the monitor output and names the output flows that carry it', () => {
    const { c, region, dest } = section();
    expect(region.getAttribute('aria-label')).toBe('Monitor output');
    expect(region.querySelector('.panel-title')!.textContent).toBe('Monitor out');
    expect(dest.textContent!.replace(/\s+/g, ' ').trim()).toBe('To Monitor out');
    expect(dest.getAttribute('aria-label')).toBe('Sent to Monitor out (out-mon)');
    unmount(c);
  });

  it('says when no output flow carries the monitor', () => {
    const { c, dest } = section((shape) => {
      shape.outputs = shape.outputs.filter((o) => o.source !== 'mon');
    });
    expect(dest.textContent!.trim()).toBe('Not on an output');
    expect(dest.getAttribute('aria-label')).toBe('No output flow carries the monitor');
    unmount(c);
  });

  it('says when the shape has no monitor output', () => {
    const { c, dest } = section((shape) => {
      shape.monitorOutputs = [];
      shape.outputs = shape.outputs.filter((o) => o.source !== 'mon');
    });
    expect(dest.textContent!.trim()).toBe('No monitor output');
    expect(dest.getAttribute('aria-label')).toBe('This shape has no monitor output');
    unmount(c);
  });
});
