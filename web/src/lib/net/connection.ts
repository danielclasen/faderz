// WebSocket link to one replica at a time. Rotates through the configured
// endpoints on failure and treats a link that stays silent longer than the
// liveness timeout as failed: a healthy replica sends meters 25 times a
// second, so silence means a hung or partitioned replica.

export interface SocketLike {
  readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type SocketFactory = (url: string) => SocketLike;

export interface ConnectionHandlers {
  open(endpoint: string): void;
  message(data: unknown): void;
  close(endpoint: string): void;
}

export interface ConnectionOptions {
  livenessMs?: number;
  backoffMs?: readonly number[];
  socketFactory?: SocketFactory;
}

const OPEN = 1;

export class Connection {
  readonly endpoints: readonly string[];
  private index = 0;
  private socket: SocketLike | null = null;
  private attempt = 0;
  private stopped = true;
  private livenessTimer: ReturnType<typeof setTimeout> | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly livenessMs: number;
  private readonly backoffMs: readonly number[];
  private readonly factory: SocketFactory;

  constructor(
    endpoints: readonly string[],
    private readonly handlers: ConnectionHandlers,
    options: ConnectionOptions = {},
  ) {
    if (endpoints.length === 0) throw new Error('no endpoints');
    this.endpoints = endpoints;
    this.livenessMs = options.livenessMs ?? 2000;
    this.backoffMs = options.backoffMs ?? [0, 250, 500, 1000, 2000, 4000];
    this.factory = options.socketFactory ?? ((url) => new WebSocket(url) as unknown as SocketLike);
  }

  get endpoint(): string {
    return this.endpoints[this.index]!;
  }

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    this.clearTimers();
    const s = this.socket;
    this.socket = null;
    s?.close(1000, 'stopped');
  }

  send(msg: unknown): boolean {
    if (!this.socket || this.socket.readyState !== OPEN) return false;
    this.socket.send(JSON.stringify(msg));
    return true;
  }

  /** Drop the current link and fail over to the next endpoint. */
  failover(): void {
    const s = this.socket;
    if (!s) return;
    this.detach(s);
    s.close(4000, 'failover');
    this.onLost();
  }

  private connect(): void {
    if (this.stopped) return;
    const endpoint = this.endpoint;
    let s: SocketLike;
    try {
      s = this.factory(endpoint);
    } catch {
      this.onLost();
      return;
    }
    this.socket = s;
    s.onopen = () => {
      this.attempt = 0;
      this.armLiveness();
      this.handlers.open(endpoint);
    };
    s.onmessage = (ev) => {
      this.armLiveness();
      this.handlers.message(ev.data);
    };
    s.onerror = () => {
      // onclose follows and handles the failure.
    };
    s.onclose = () => {
      if (this.socket !== s) return;
      this.detach(s);
      this.onLost();
    };
  }

  private detach(s: SocketLike): void {
    s.onopen = s.onmessage = s.onclose = s.onerror = null;
    if (this.socket === s) this.socket = null;
    if (this.livenessTimer) clearTimeout(this.livenessTimer);
    this.livenessTimer = null;
  }

  private onLost(): void {
    const endpoint = this.endpoint;
    this.index = (this.index + 1) % this.endpoints.length;
    this.handlers.close(endpoint);
    if (this.stopped) return;
    const delay = this.backoffMs[Math.min(this.attempt, this.backoffMs.length - 1)]!;
    this.attempt++;
    this.retryTimer = setTimeout(() => this.connect(), delay);
  }

  private armLiveness(): void {
    if (this.livenessTimer) clearTimeout(this.livenessTimer);
    this.livenessTimer = setTimeout(() => this.failover(), this.livenessMs);
  }

  private clearTimers(): void {
    if (this.livenessTimer) clearTimeout(this.livenessTimer);
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.livenessTimer = this.retryTimer = null;
  }
}

/** Resolve configured endpoints against the page location. */
export function resolveEndpoints(param: string | null, location: { protocol: string; host: string }): string[] {
  const list = (param ?? '/api/v1/ws')
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return list.map((e) => (/^wss?:\/\//.test(e) ? e : `${scheme}//${location.host}${e.startsWith('/') ? '' : '/'}${e}`));
}

/** REST base URL served by the same replica as a WebSocket endpoint. */
export function restBase(endpoint: string): string {
  return endpoint.replace(/^ws/, 'http').replace(/\/ws$/, '');
}
