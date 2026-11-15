// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import StatusBar from '../../src/components/StatusBar.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { SyncStatus } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const status = (commitable: boolean): SyncStatus => ({
  ...INITIAL_STATUS,
  link: 'Live',
  primaryReplica: 'r1',
  replicas: [{ id: 'r1', role: 'leader', healthy: true, commitable }],
});

describe('notices while nothing commits', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('keeps notices raised under the commit banner until the line shows them again, then times them', () => {
    const s = new MixerStore();
    s.attach({ post: () => {} });
    const shape = exampleShape();
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: status(false) });
    const target = document.createElement('div');
    document.body.append(target);
    const c = mount(StatusBar, { target, context: mixerContext(s) });
    flushSync();
    expect(target.querySelector('[data-testid="commit-banner"]')).not.toBeNull();
    s.notify('info', 'Not confirmed in time, not sent again: Pres 1 mute');
    flushSync();
    vi.advanceTimersByTime(20_000);
    expect(s.notices.map((n) => n.text)).toEqual(['Not confirmed in time, not sent again: Pres 1 mute']);
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status(true) });
    flushSync();
    expect(target.querySelector('[data-testid="notice"]')?.textContent).toContain('Pres 1 mute');
    vi.advanceTimersByTime(4000);
    flushSync();
    expect(s.notices).toEqual([]);
    unmount(c);
  });
});

describe('commit banner', () => {
  it('is a status, not an alert, and its name carries no count that changes', () => {
    const s = new MixerStore();
    s.attach({ post: () => {} });
    const shape = exampleShape();
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: { ...status(false), outbox: 3 } });
    const target = document.createElement('div');
    document.body.append(target);
    const c = mount(StatusBar, { target, context: mixerContext(s) });
    flushSync();
    const banner = target.querySelector<HTMLElement>('[data-testid="commit-banner"]')!;
    expect(banner.getAttribute('role')).toBe('status');
    const name = banner.getAttribute('aria-label')!;
    expect(name).toMatch(/not committing/i);
    expect(name).not.toMatch(/\d/);
    expect(target.querySelector('[data-testid="commit-pending"]')!.getAttribute('aria-hidden')).toBe('true');
    expect(target.querySelector('[data-testid="commit-pending"]')!.textContent).toContain('3');
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: { ...status(false), outbox: 5 } });
    flushSync();
    expect(banner.getAttribute('aria-label')).toBe(name);
    unmount(c);
  });
});
