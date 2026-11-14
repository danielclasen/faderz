// REST calls with failover: a request that fails on the network or with a
// gateway error is retried on the next endpoint with the same command id,
// which the server applies at most once.

import type { ApiError } from '../contract/types';

export class ApiRequestError extends Error {
  constructor(
    readonly status: number,
    readonly body: ApiError | null,
  ) {
    super(body?.message ?? `HTTP ${status}`);
  }
}

const RETRY_STATUS = new Set([502, 503, 504]);

export class RestClient {
  constructor(
    private readonly bases: () => string[],
    private readonly attempts = 4,
    private readonly retryDelayMs = 300,
  ) {}

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const bases = this.bases();
    let lastError: unknown = null;
    for (let i = 0; i < this.attempts; i++) {
      const base = bases[i % bases.length]!;
      if (i > 0) await new Promise((r) => setTimeout(r, this.retryDelayMs));
      let res: Response;
      try {
        res = await fetch(base + path, {
          method,
          headers:
            body === undefined
              ? { 'X-Samplez-Request': '1' }
              : { 'Content-Type': 'application/json', 'X-Samplez-Request': '1' },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
      } catch (e) {
        lastError = e;
        continue;
      }
      if (RETRY_STATUS.has(res.status)) {
        lastError = new ApiRequestError(res.status, null);
        continue;
      }
      const text = await res.text();
      const json = text ? (JSON.parse(text) as unknown) : null;
      if (!res.ok) throw new ApiRequestError(res.status, json as ApiError | null);
      return json as T;
    }
    throw lastError instanceof Error ? lastError : new Error('request failed');
  }
}
