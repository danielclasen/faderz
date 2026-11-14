import { describe, expect, it } from 'vitest';
import { resolveEndpoints, restBase } from '../../src/lib/sync/endpoints';

const page = { protocol: 'http:', host: 'mixer.local:8080' };
const secure = { protocol: 'https:', host: 'mixer.local' };

describe('resolveEndpoints', () => {
  it('serves the default endpoint on the page origin', () => {
    expect(resolveEndpoints(null, page)).toEqual(['ws://mixer.local:8080/api/v1/ws']);
    expect(resolveEndpoints(null, secure)).toEqual(['wss://mixer.local/api/v1/ws']);
  });

  it('takes paths and absolute URLs of the page origin', () => {
    expect(resolveEndpoints('/a/ws, b/ws,ws://mixer.local:8080/c/ws', page)).toEqual([
      'ws://mixer.local:8080/a/ws',
      'ws://mixer.local:8080/b/ws',
      'ws://mixer.local:8080/c/ws',
    ]);
    expect(resolveEndpoints('wss://mixer.local/r1/ws', secure)).toEqual(['wss://mixer.local/r1/ws']);
  });

  it.each([
    ['another host', 'ws://evil.example/api/v1/ws', page],
    ['another port', 'ws://mixer.local:9090/api/v1/ws', page],
    ['a host that starts like the page host', 'ws://mixer.local:8080.evil.example/ws', page],
    ['credentials in front of the page host', 'ws://mixer.local:8080@evil.example/ws', page],
    ['plain ws from an https page', 'ws://mixer.local/api/v1/ws', secure],
    ['another scheme', 'http://mixer.local:8080/api/v1/ws', page],
    ['a protocol-relative URL', '//evil.example/api/v1/ws', page],
  ])('refuses %s', (_, endpoints, location) => {
    expect(() => resolveEndpoints(`/api/v1/ws,${endpoints}`, location)).toThrow(/not on this origin/);
  });
});

describe('restBase', () => {
  it('maps a WebSocket endpoint to the REST base of the same replica', () => {
    expect(restBase('wss://mixer.local/__mock/replicas/r1/api/v1/ws')).toBe('https://mixer.local/__mock/replicas/r1/api/v1');
  });
});
