// REST calls with failover: a request that fails on the network, times
// out or answers with a gateway error is retried on the next endpoint
// with the same command id, which the server applies at most once.

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
/** A request without a response by then is abandoned and tried on the next endpoint. */
export const REST_TIMEOUT_MS = 3000;

export class RestClient {
  constructor(
    private readonly bases: () => string[],
    private readonly attempts = 4,
    private readonly retryDelayMs = 300,
    private readonly timeoutMs = REST_TIMEOUT_MS,
    /** Called on a 401: the session ended, the caller signs in again. */
    private readonly onUnauthorized: () => void = () => {},
  ) {}

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const bases = this.bases();
    let lastError: unknown = null;
    /** An earlier attempt may have applied: its answer was lost or timed out. */
    let mayHaveApplied = false;
    for (let i = 0; i < this.attempts; i++) {
      const base = bases[i % bases.length]!;
      if (i > 0) await new Promise((r) => setTimeout(r, this.retryDelayMs));
      const abort = new AbortController();
      const timer = setTimeout(() => abort.abort(new Error(`no response within ${this.timeoutMs} ms`)), this.timeoutMs);
      let res: Response;
      let text: string;
      try {
        res = await fetch(base + path, {
          method,
          // Every state-changing request carries both, a body-less DELETE
          // too: the server refuses one without them (api/openapi.yaml).
          headers: { 'Content-Type': 'application/json', 'X-Samplez-Request': '1' },
          body: body === undefined ? undefined : JSON.stringify(body),
          signal: abort.signal,
        });
        text = await res.text();
      } catch (e) {
        // Network error or timeout: the request may or may not have applied.
        lastError = abort.signal.aborted ? abort.signal.reason : e;
        mayHaveApplied = true;
        continue;
      } finally {
        clearTimeout(timer);
      }
      if (RETRY_STATUS.has(res.status)) {
        lastError = new ApiRequestError(res.status, null);
        // 503 is the server's own not applied; a gateway error or timeout
        // may come after the request reached it.
        if (res.status !== 503) mayHaveApplied = true;
        continue;
      }
      // A DELETE retried after an attempt whose answer was lost finds
      // nothing left to delete: that attempt applied. After only
      // not-applied answers the 404 is someone else's delete.
      if (method === 'DELETE' && res.status === 404 && mayHaveApplied) return null as T;
      const json = parseJson(text);
      if (res.status === 401) this.onUnauthorized();
      if (!res.ok) throw new ApiRequestError(res.status, isApiError(json) ? json : null);
      if (json === undefined) throw new Error(`HTTP ${res.status}: response is not JSON`);
      return json as T;
    }
    throw lastError instanceof Error ? lastError : new Error('request failed');
  }
}

/** The body as JSON; undefined when it is not JSON, as the error page of a proxy. */
function parseJson(text: string): unknown {
  if (!text) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

function isApiError(v: unknown): v is ApiError {
  return typeof v === 'object' && v !== null && typeof (v as { message?: unknown }).message === 'string';
}
