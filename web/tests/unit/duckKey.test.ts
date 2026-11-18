// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import DuckKey from '../../src/components/DuckKey.svelte';
import DuckShortcuts from '../../src/components/DuckShortcuts.svelte';
import { DuckPress, LATCH_MS, groupRefusals, targetRefusal, triggerRefusal } from '../../src/lib/duck';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { Owner } from '../../src/lib/contract/types';
import type { ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const HOLD = '/ducks/comm/hold';
const MODE = '/ducks/comm/mode';
const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };
const NO_QUORUM = { ...LIVE, primaryReplica: 'r1', replicas: [{ id: 'r1', role: 'leader' as const, healthy: true, commitable: false }] };
const OTHER: Owner = { principal: 'anonymous', handle: 'b3RoZXItc2Vzc2lvbi0xMjM', kind: 'ui', connection: 'c9' };

/** A store with the example state, `changes` applied, recording what it sends. */
function store(changes: Array<[string, unknown]> = [], owners: Array<[string, Owner | null]> = []) {
  const s = new MixerStore();
  const posted: ToWorker[] = [];
  s.attach({ post: (m) => posted.push(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes, owners, status: LIVE });
  return { s, posted, shape };
}

/** DuckPress of group comm, with explicit press and lift times. */
function press(changes: Array<[string, unknown]> = [], owners: Array<[string, Owner | null]> = []) {
  const t = store(changes, owners);
  return { ...t, key: new DuckPress(t.s, 'comm', () => 'Commentary') };
}

const HELD = [
  { type: 'touch', path: HOLD },
  { type: 'set', path: HOLD, value: true },
];
const ENDED = [
  { type: 'set', path: HOLD, value: false },
  { type: 'release', path: HOLD },
];

describe('DuckPress', () => {
  it('holds the duck in from the press, and a press of LATCH_MS or longer only ends it', () => {
    const { posted, key } = press();
    key.press(1000);
    expect(posted).toEqual(HELD);
    expect(key.held).toBe(true);
    key.lift(1000 + LATCH_MS);
    expect(posted).toEqual([...HELD, ...ENDED]);
    expect(key.held).toBe(false);
  });

  it('latches mode on with a tap, before the hold ends, so the duck never drops between them', () => {
    const { posted, key } = press();
    key.press(1000);
    key.lift(1000 + LATCH_MS - 1);
    expect(posted).toEqual([...HELD, { type: 'set', path: MODE, value: 'on' }, ...ENDED]);
  });

  it('sets a latched group back to auto with a tap', () => {
    const { posted, key } = press([[MODE, 'on']]);
    key.press(0);
    key.lift(10);
    expect(posted).toEqual([...HELD, { type: 'set', path: MODE, value: 'auto' }, ...ENDED]);
  });

  it('sends nothing for a tap on a bypassed group and says so, and holds it in only once a press lasts LATCH_MS', () => {
    vi.useFakeTimers();
    try {
      const { s, posted, key } = press([[MODE, 'off']]);
      key.press(0);
      vi.advanceTimersByTime(LATCH_MS - 1);
      key.lift(LATCH_MS - 1);
      vi.runAllTimers();
      expect(posted).toEqual([]);
      expect(s.notices.map((n) => [n.kind, n.text])).toEqual([['info', 'Commentary is bypassed: choose AUTO or ON to arm it']]);
      key.press(1000);
      expect(posted).toEqual([]);
      vi.advanceTimersByTime(LATCH_MS);
      expect(posted).toEqual(HELD);
      key.lift(1000 + 2 * LATCH_MS);
      expect(posted).toEqual([...HELD, ...ENDED]);
      // An interrupted press before then sends nothing either.
      posted.length = 0;
      key.press(5000);
      key.abort();
      vi.runAllTimers();
      expect(posted).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('holds nothing while another session holds the duck, names the holder, and still toggles mode with a tap', () => {
    const { s, posted, key } = press([[HOLD, true]], [[HOLD, OTHER]]);
    key.press(0);
    expect(posted).toEqual([]);
    expect(s.notices.map((n) => [n.kind, n.text])).toEqual([['conflict', 'Commentary duck hold is held by web client b3RoZX']]);
    key.lift(10);
    expect(posted).toEqual([{ type: 'set', path: MODE, value: 'on' }]);
    // A long press of that kind writes nothing at all.
    posted.length = 0;
    key.press(100);
    key.lift(100 + LATCH_MS);
    expect(posted).toEqual([]);
  });

  it('starts no hold while nothing commits, but a tap still latches', () => {
    const { s, posted, key } = press();
    s.apply({ type: 'patch', input: 1, changes: [], owners: [], status: NO_QUORUM });
    key.press(0);
    expect(posted).toEqual([]);
    key.lift(10);
    expect(posted).toEqual([{ type: 'set', path: MODE, value: 'on' }]);
  });

  it('while nothing commits, ends a hold with the release alone, so the screen never shows it ended early', () => {
    const { s, posted, key } = press();
    key.press(0);
    s.apply({ type: 'patch', input: 1, changes: [], owners: [], status: NO_QUORUM });
    key.lift(LATCH_MS);
    expect(posted).toEqual([...HELD, { type: 'release', path: HOLD }]);
  });

  it('a bypassed group: re-checks the holder when the deferred hold starts, and holds nothing another session took meanwhile', () => {
    vi.useFakeTimers();
    try {
      const { s, posted, key } = press([[MODE, 'off']]);
      key.press(0);
      s.apply({ type: 'patch', input: 1, changes: [[HOLD, true]], owners: [[HOLD, OTHER]], status: LIVE });
      vi.advanceTimersByTime(LATCH_MS);
      expect(posted).toEqual([]);
      expect(s.notices.map((n) => n.kind)).toEqual(['conflict']);
      key.lift(2 * LATCH_MS);
      expect(posted).toEqual([]);
      expect(s.value(HOLD)).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('a bypassed group: a press that lasted LATCH_MS holds and lets go even when the deferred start has not run', () => {
    vi.useFakeTimers();
    try {
      const { posted, key } = press([[MODE, 'off']]);
      key.press(0);
      // A busy page: the timer has not fired by the release.
      key.lift(LATCH_MS);
      expect(posted).toEqual([...HELD, ...ENDED]);
      vi.runAllTimers();
      expect(posted).toHaveLength(4);
    } finally {
      vi.useRealTimers();
    }
  });

  it('clears no hold another session holds by the end of the press, and only releases', () => {
    const { s, posted, key } = press();
    key.press(0);
    // Given up on request meanwhile, and taken by another session.
    s.apply({ type: 'patch', input: 1, changes: [[HOLD, true]], owners: [[HOLD, OTHER]], status: LIVE });
    key.lift(LATCH_MS);
    expect(posted).toEqual([...HELD, { type: 'release', path: HOLD }]);
    expect(s.value(HOLD)).toBe(true);
  });

  it('keeps the hold until the last of two keys of the group lets go', () => {
    const { s, posted } = press();
    const tile = new DuckPress(s, 'comm', () => 'Commentary');
    const pill = new DuckPress(s, 'comm', () => 'Commentary');
    tile.press(0);
    pill.press(10);
    expect(posted).toEqual(HELD);
    tile.lift(LATCH_MS);
    expect(posted).toEqual(HELD);
    pill.lift(10 + LATCH_MS);
    expect(posted).toEqual([...HELD, ...ENDED]);
    // And a new press holds again from the start.
    posted.length = 0;
    tile.press(1000);
    expect(posted).toEqual(HELD);
    tile.abort();
  });

  it('does nothing while writes are locked', () => {
    const { s, posted, key } = press();
    s.deskLock = true;
    key.press(0);
    key.lift(10);
    expect(posted).toEqual([]);
  });

  it('ends an interrupted press without a mode change, however short', () => {
    const { posted, key } = press();
    key.press(0);
    key.abort();
    key.lift(10);
    expect(posted).toEqual([...HELD, ...ENDED]);
  });
});

describe('DUCK key', () => {
  let now = 0;
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });

  function key(changes: Array<[string, unknown]> = []) {
    const t = store(changes);
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    const target = document.createElement('div');
    document.body.append(target);
    const c = mount(DuckKey, { target, props: { id: 'comm', name: 'Commentary', shortcut: 1 }, context: mixerContext(t.s) });
    flushSync();
    const button = target.querySelector('button')!;
    return { ...t, c, button };
  }
  const keyboard = (el: EventTarget, type: 'keydown' | 'keyup', init: KeyboardEventInit) => el.dispatchEvent(new KeyboardEvent(type, { bubbles: true, ...init }));

  it('holds with Space for as long as it is down, and latches with a short Space tap', () => {
    const { posted, button, c } = key();
    now = 0;
    keyboard(button, 'keydown', { key: ' ' });
    now = LATCH_MS;
    keyboard(button, 'keyup', { key: ' ' });
    expect(posted).toEqual([...HELD, ...ENDED]);
    posted.length = 0;
    now = 1000;
    keyboard(button, 'keydown', { key: ' ' });
    now = 1050;
    keyboard(button, 'keyup', { key: ' ' });
    expect(posted).toEqual([...HELD, { type: 'set', path: MODE, value: 'on' }, ...ENDED]);
    flushSync();
    expect(button.getAttribute('aria-label')).toBe('Commentary duck, latched, Shift+D+1');
    expect(button.getAttribute('aria-pressed')).toBe('true');
    unmount(c);
  });

  it('ends a hold on a cancelled touch, a lost capture, a blur, a hidden or left page, and changes no mode', () => {
    for (const end of [
      (b: HTMLElement) => b.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true })),
      (b: HTMLElement) => b.dispatchEvent(new PointerEvent('lostpointercapture', { bubbles: true })),
      (b: HTMLElement) => b.dispatchEvent(new FocusEvent('blur')),
      () => window.dispatchEvent(new Event('pagehide')),
      () => {
        Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
        document.dispatchEvent(new Event('visibilitychange'));
        delete (document as { visibilityState?: unknown }).visibilityState;
      },
    ]) {
      const { posted, button, c } = key();
      now = 0;
      keyboard(button, 'keydown', { key: 'Enter' });
      end(button);
      // The release that follows is no press of its own: no latch.
      keyboard(button, 'keyup', { key: 'Enter' });
      expect(posted).toEqual([...HELD, ...ENDED]);
      unmount(c);
      document.body.replaceChildren();
    }
  });

  it('shows on its face who holds the group, and DUCK otherwise', () => {
    const t = store([[HOLD, true]], [[HOLD, OTHER]]);
    const target = document.body.appendChild(document.createElement('div'));
    const c = mount(DuckKey, { target, props: { id: 'comm', name: 'Commentary' }, context: mixerContext(t.s) });
    flushSync();
    const button = target.querySelector('button')!;
    expect(button.textContent!.trim()).toBe('b3RoZX');
    expect(button.getAttribute('aria-label')).toBe('Commentary duck, held by web client b3RoZX');
    t.s.apply({ type: 'patch', input: 1, changes: [[HOLD, false]], owners: [[HOLD, null]], status: LIVE });
    flushSync();
    expect(button.textContent!.trim()).toBe('DUCK');
    unmount(c);
  });

  it('ends a hold when the key leaves the screen', async () => {
    const { posted, button, c } = key();
    keyboard(button, 'keydown', { key: ' ' });
    unmount(c);
    flushSync();
    await Promise.resolve();
    expect(posted).toEqual([...HELD, ...ENDED]);
  });

  it('holds with Shift+D+its digit while the digit is down, never latches with it, and not while typing', () => {
    const t = store();
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    const target = document.body.appendChild(document.createElement('div'));
    const c = mount(DuckShortcuts, { target, props: {}, context: mixerContext(t.s) });
    flushSync();
    const posted = t.posted;
    now = 0;
    keyboard(window, 'keydown', { key: 'D', code: 'KeyD', shiftKey: true });
    keyboard(window, 'keydown', { key: '!', code: 'Digit1', shiftKey: true });
    expect(posted).toEqual(HELD);
    // However short, the chord only held: a slip of three keys leaves nothing on.
    now = 10;
    keyboard(window, 'keyup', { key: '!', code: 'Digit1', shiftKey: true });
    expect(posted).toEqual([...HELD, ...ENDED]);
    // Another digit is another group's, a digit with no group nobody's.
    posted.length = 0;
    keyboard(window, 'keydown', { key: '@', code: 'Digit2', shiftKey: true });
    expect(posted).toEqual([
      { type: 'touch', path: '/ducks/remote/hold' },
      { type: 'set', path: '/ducks/remote/hold', value: true },
    ]);
    keyboard(window, 'keyup', { key: '@', code: 'Digit2', shiftKey: true });
    posted.length = 0;
    keyboard(window, 'keydown', { key: '#', code: 'Digit3', shiftKey: true });
    expect(posted).toEqual([]);
    // Letting go of D ends the press.
    keyboard(window, 'keydown', { key: '!', code: 'Digit1', shiftKey: true });
    now = 2 * LATCH_MS;
    keyboard(window, 'keyup', { key: 'D', code: 'KeyD', shiftKey: true });
    expect(posted).toEqual([...HELD, ...ENDED]);
    // In a text field the keys are text.
    posted.length = 0;
    const input = document.body.appendChild(document.createElement('input'));
    keyboard(input, 'keydown', { key: 'D', code: 'KeyD', shiftKey: true });
    keyboard(input, 'keydown', { key: '!', code: 'Digit1', shiftKey: true });
    expect(posted).toEqual([]);
    unmount(c);
  });
});

describe('groupRefusals', () => {
  it('says what triggerRefusal and targetRefusal say for every channel, from one count of the triggers', () => {
    const shape = exampleShape();
    const state = initialState(shape);
    // A full group: eight triggers.
    for (const id of ['ch3', 'ch4', 'ch5', 'ch6', 'ch7', 'ch8']) state.ducks['comm']!.channels[id]!.trigger = true;
    let reads = 0;
    const channels = state.ducks['comm']!.channels;
    state.ducks['comm']!.channels = new Proxy(channels, {
      get(t, k) {
        reads++;
        return t[k as string];
      },
    });
    const got = groupRefusals(shape, state, 'comm');
    // One read per channel to count, one to judge: no read per pair of channels.
    expect(reads).toBeLessThanOrEqual(2 * shape.channels.length);
    state.ducks['comm']!.channels = channels;
    for (const c of shape.channels) {
      expect(got.get(c.id)).toEqual({ trigger: triggerRefusal(shape, state, 'comm', c.id), target: targetRefusal(shape, state, 'comm', c.id) });
    }
    expect(got.get('ch9')!.trigger).toBe('Commentary takes at most 8 triggers');
    expect(got.get('ch13')!.trigger).toMatch(/is a target of Commentary/);
    expect(got.get('ch1')!.target).toMatch(/triggers Commentary/);
  });
});
