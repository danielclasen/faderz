import { afterEach, describe, expect, it, vi } from 'vitest';
import { uuid } from '../../src/lib/net/ids';
import { sessionId } from '../../src/lib/sync/session';

const V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

describe('uuid on a non-secure origin', () => {
  afterEach(() => vi.unstubAllGlobals());

  // Plain-HTTP origins have crypto.getRandomValues but no crypto.randomUUID.
  it('generates distinct RFC 4122 v4 UUIDs without crypto.randomUUID', () => {
    const real = globalThis.crypto;
    vi.stubGlobal('crypto', { getRandomValues: (a: Uint8Array<ArrayBuffer>) => real.getRandomValues(a) });
    expect((globalThis.crypto as { randomUUID?: unknown }).randomUUID).toBeUndefined();
    const ids = Array.from({ length: 1000 }, uuid);
    for (const id of ids) expect(id).toMatch(V4);
    expect(new Set(ids).size).toBe(1000);
  });

  it('creates a session id without crypto.randomUUID', () => {
    const real = globalThis.crypto;
    vi.stubGlobal('crypto', { getRandomValues: (a: Uint8Array<ArrayBuffer>) => real.getRandomValues(a) });
    expect(sessionId(null, 'navigate')).toMatch(/^[A-Za-z0-9_-]{22}$/);
  });
});
