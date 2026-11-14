import { describe, expect, it } from 'vitest';
import { sessionId, sessionTag } from '../../src/lib/sync/session';

class MemoryStorage {
  data = new Map<string, string>();
  getItem(k: string): string | null {
    return this.data.get(k) ?? null;
  }
  setItem(k: string, v: string): void {
    this.data.set(k, v);
  }
}

let n = 0;
const gen = () => `session-${++n}`;

describe('session identity', () => {
  it('keeps the stored session across a reload of the tab', () => {
    const s = new MemoryStorage();
    const first = sessionId(s, 'navigate', gen);
    expect(sessionId(s, 'reload', gen)).toBe(first);
    expect(sessionId(s, 'reload', gen)).toBe(first);
  });

  it('starts a new session on a navigation or a duplicated tab, which copies sessionStorage', () => {
    const s = new MemoryStorage();
    const first = sessionId(s, 'navigate', gen);
    const duplicate = new MemoryStorage();
    duplicate.data = new Map(s.data);
    const dup = sessionId(duplicate, 'navigate', gen);
    expect(dup).not.toBe(first);
    expect(sessionId(s, 'back_forward', gen)).not.toBe(first);
    expect(sessionId(s, undefined, gen)).not.toBe(first);
  });

  it('works without storage and when storage throws', () => {
    expect(sessionId(null, 'reload', gen)).toMatch(/^session-/);
    const broken = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };
    const a = sessionId(broken, 'reload', gen);
    const b = sessionId(broken, 'reload', gen);
    expect(a).not.toBe(b);
  });

  it('shows a short tag of a session id', () => {
    expect(sessionTag('0f3a9c2e-1111-4222-8333-444455556666')).toBe('0f3a9c');
  });
});
