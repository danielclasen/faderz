import { createHash, randomBytes } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { handleTag, ownerHandle, sha256 } from '../../src/lib/net/handle';
import { randomSessionId } from '../../src/lib/net/ids';

const hex = (b: Uint8Array) => Buffer.from(b).toString('hex');

describe('sha256', () => {
  it('matches the FIPS 180-4 examples', () => {
    expect(hex(sha256(new TextEncoder().encode('abc')))).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
    expect(hex(sha256(new Uint8Array(0)))).toBe('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
  });

  it('matches node:crypto across every padding boundary', () => {
    for (let n = 0; n <= 200; n++) {
      const data = randomBytes(n);
      expect(hex(sha256(new Uint8Array(data))), `length ${n}`).toBe(createHash('sha256').update(data).digest('hex'));
    }
  });
});

describe('ownerHandle', () => {
  it('is base64url of the first 16 bytes of SHA-256 over the session id, 22 characters', () => {
    for (let i = 0; i < 50; i++) {
      const session = randomSessionId();
      const expected = createHash('sha256').update(session, 'ascii').digest().subarray(0, 16).toString('base64url');
      expect(ownerHandle(session)).toBe(expected);
      expect(ownerHandle(session)).toMatch(/^[A-Za-z0-9_-]{22}$/);
    }
  });
});

describe('handleTag', () => {
  it('shows the first six characters of a handle other than - and _', () => {
    expect(handleTag('_0f-3a9c2e1111422283334')).toBe('0f3a9c');
  });
});
