// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { hold } from '../../src/lib/hold';

function key() {
  const node = document.createElement('button');
  node.setPointerCapture = () => {};
  document.body.append(node);
  return node;
}
const down = (n: HTMLElement) => n.dispatchEvent(new PointerEvent('pointerdown', { button: 0, pointerId: 1 }));
const up = (n: HTMLElement) => n.dispatchEvent(new PointerEvent('pointerup', { button: 0, pointerId: 1 }));

describe('hold', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('fires on the press itself for a hold of 0 ms, by pointer and by key, with no arming', () => {
    const fire = vi.fn();
    const n = key();
    hold(n, { ms: 0, fire });
    down(n);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(n.classList.contains('arming')).toBe(false);
    up(n);
    n.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }));
    expect(fire).toHaveBeenCalledTimes(2);
    n.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', repeat: true }));
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('fires a longer hold only once it is held through, and not when let go early', () => {
    const fire = vi.fn();
    const n = key();
    hold(n, { ms: 500, fire });
    down(n);
    expect(n.classList.contains('arming')).toBe(true);
    vi.advanceTimersByTime(499);
    up(n);
    vi.advanceTimersByTime(10);
    expect(fire).not.toHaveBeenCalled();
    down(n);
    vi.advanceTimersByTime(500);
    expect(fire).toHaveBeenCalledTimes(1);
    expect(n.classList.contains('arming')).toBe(false);
  });

  it('fires what was given when the press started, whatever an update gives during the hold', () => {
    const first = vi.fn();
    const second = vi.fn();
    const n = key();
    const action = hold(n, { ms: 500, fire: first });
    down(n);
    action.update({ ms: 0, fire: second });
    vi.advanceTimersByTime(500);
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
  });
});
