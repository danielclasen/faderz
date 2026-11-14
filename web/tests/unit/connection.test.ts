import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Connection, resolveEndpoints, restBase, type SocketLike } from '../../src/lib/net/connection';

class FakeSocket implements SocketLike {
  readyState = 0;
  sent: string[] = [];
  onopen: SocketLike['onopen'] = null;
  onmessage: SocketLike['onmessage'] = null;
  onclose: SocketLike['onclose'] = null;
  onerror: SocketLike['onerror'] = null;
  constructor(readonly url: string) {}
  send(d: string) {
    this.sent.push(d);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  message(d: string) {
    this.onmessage?.({ data: d });
  }
}

describe('Connection', () => {
  let sockets: FakeSocket[];
  let events: string[];
  beforeEach(() => {
    vi.useFakeTimers();
    sockets = [];
    events = [];
  });
  afterEach(() => vi.useRealTimers());

  const make = (endpoints: string[]) =>
    new Connection(
      endpoints,
      {
        open: (e) => events.push(`open ${e}`),
        message: (d) => events.push(`msg ${String(d)}`),
        close: (e) => events.push(`close ${e}`),
      },
      {
        livenessMs: 2000,
        backoffMs: [0, 100],
        socketFactory: (url) => {
          const s = new FakeSocket(url);
          sockets.push(s);
          return s;
        },
      },
    );

  it('moves to the next endpoint when a link closes', async () => {
    const c = make(['ws://a', 'ws://b']);
    c.start();
    sockets[0]!.open();
    sockets[0]!.close();
    await vi.advanceTimersByTimeAsync(0);
    expect(sockets[1]!.url).toBe('ws://b');
    expect(events).toEqual(['open ws://a', 'close ws://a']);
  });

  it('fails over when an open link goes silent past the liveness timeout', async () => {
    const c = make(['ws://a', 'ws://b']);
    c.start();
    sockets[0]!.open();
    await vi.advanceTimersByTimeAsync(1500);
    sockets[0]!.message('m');
    await vi.advanceTimersByTimeAsync(1500);
    expect(sockets).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(600);
    expect(events).toContain('close ws://a');
    expect(sockets[1]?.url).toBe('ws://b');
    expect(c.send({})).toBe(false);
  });

  it('backs off between failed attempts and stops on stop()', async () => {
    const c = make(['ws://a']);
    c.start();
    sockets[0]!.close();
    await vi.advanceTimersByTimeAsync(0);
    expect(sockets).toHaveLength(2);
    sockets[1]!.close();
    await vi.advanceTimersByTimeAsync(50);
    expect(sockets).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(60);
    expect(sockets).toHaveLength(3);
    c.stop();
    await vi.advanceTimersByTimeAsync(1000);
    expect(sockets).toHaveLength(3);
  });
});

describe('endpoints', () => {
  it('defaults to the same-origin service endpoint and resolves relative paths', () => {
    expect(resolveEndpoints(null, { protocol: 'https:', host: 'mix.example' })).toEqual(['wss://mix.example/api/v1/ws']);
    expect(resolveEndpoints('ws://r1:8080/api/v1/ws, /x/api/v1/ws', { protocol: 'http:', host: 'h' })).toEqual([
      'ws://r1:8080/api/v1/ws',
      'ws://h/x/api/v1/ws',
    ]);
  });

  it('derives the REST base of the same replica', () => {
    expect(restBase('wss://h/api/v1/ws')).toBe('https://h/api/v1');
  });
});
