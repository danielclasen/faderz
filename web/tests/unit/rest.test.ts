import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiRequestError, RestClient } from '../../src/lib/net/rest';

type Reply = 'hang' | 'network' | { status: number; body?: string };

/** fetch answering each call with the next reply; a hung call ends only when aborted. */
function stubFetch(...replies: Reply[]) {
  const calls: Array<{ url: string; method: string }> = [];
  vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
    calls.push({ url, method: init.method ?? 'GET' });
    const reply = replies.shift() ?? 'network';
    if (reply === 'network') return Promise.reject(new TypeError('Failed to fetch'));
    if (reply === 'hang') {
      return new Promise((_, reject) => init.signal?.addEventListener('abort', () => reject(init.signal!.reason)));
    }
    return Promise.resolve(new Response(reply.body ?? null, { status: reply.status }));
  });
  return calls;
}

const client = () => new RestClient(() => ['http://a/api/v1', 'http://b/api/v1'], 4, 0, 50);

afterEach(() => vi.unstubAllGlobals());

describe('RestClient', () => {
  it('abandons a request without a response after the timeout and retries it on the next endpoint', async () => {
    const calls = stubFetch('hang', { status: 200, body: '{"result":"applied","revision":3}' });
    const started = Date.now();
    await expect(client().request('POST', '/scenes/s1/recall', { commandId: 'x' })).resolves.toEqual({ result: 'applied', revision: 3 });
    expect(Date.now() - started).toBeLessThan(1000);
    expect(calls.map((c) => c.url)).toEqual(['http://a/api/v1/scenes/s1/recall', 'http://b/api/v1/scenes/s1/recall']);
  });

  it('fails with the timeout when no endpoint answers', async () => {
    stubFetch('hang', 'hang', 'hang', 'hang');
    await expect(client().request('GET', '/state')).rejects.toThrow(/no response within 50 ms/);
  });

  it('takes a 404 to a retried DELETE as the delete that applied on the lost attempt', async () => {
    stubFetch('network', { status: 404, body: '{"code":404,"message":"unknown scene s1"}' });
    await expect(client().request('DELETE', '/scenes/s1')).resolves.toBeNull();
  });

  it('reports a 404 to a first DELETE', async () => {
    stubFetch({ status: 404, body: '{"code":404,"message":"unknown scene s1"}' });
    await expect(client().request('DELETE', '/scenes/s1')).rejects.toMatchObject({ status: 404, message: 'unknown scene s1' });
  });

  it('reports an error page that is not JSON as an ApiRequestError with its status', async () => {
    stubFetch({ status: 500, body: '<html>Internal Server Error</html>' });
    const e = await client().request('GET', '/state').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(ApiRequestError);
    expect(e).toMatchObject({ status: 500, body: null, message: 'HTTP 500' });
  });
});
