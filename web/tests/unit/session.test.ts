import { describe, expect, it } from 'vitest';
import { sessionId, sessionTag, withSession } from '../../src/lib/sync/session';
import { randomSessionId } from '../../src/lib/net/ids';

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
const gen = () => `session-${++n}-0123456789abcdef`;

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

describe('session id format', () => {
  it('is 128 random bits as 22 base64url characters without padding', () => {
    const ids = Array.from({ length: 200 }, randomSessionId);
    for (const id of ids) expect(id).toMatch(/^[A-Za-z0-9_-]{22}$/);
    expect(new Set(ids).size).toBe(ids.length);
    const decoded = Buffer.from(ids[0]!, 'base64url');
    expect(decoded.length).toBe(16);
  });

  it('replaces a stored id that is not a SessionId on reload', () => {
    const s = new Map<string, string>([['faderz.session', 'e3b0c442-98fc-4c14-9afb-f4c8996fb924']]);
    const storage = { getItem: (k: string) => s.get(k) ?? null, setItem: (k: string, v: string) => void s.set(k, v) };
    const id = sessionId(storage, 'reload');
    expect(id).toMatch(/^[A-Za-z0-9_-]{22,128}$/);
    expect(s.get('faderz.session')).toBe(id);
  });

  it('adds ?session= to an endpoint, keeping its own query', () => {
    expect(withSession('wss://h/api/v1/ws', 'abc_DEF-0123456789abcd')).toBe('wss://h/api/v1/ws?session=abc_DEF-0123456789abcd');
    expect(withSession('ws://h/ws?x=1', 'abc_DEF-0123456789abcd')).toBe('ws://h/ws?x=1&session=abc_DEF-0123456789abcd');
  });
});
