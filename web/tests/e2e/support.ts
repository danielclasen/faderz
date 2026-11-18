// Shared helpers of the e2e specs: REST and mock controls, page waits,
// pointer drags, and a raw WebSocket client speaking the contract.

import { expect, type APIRequestContext, type Page } from '@playwright/test';
import WebSocket from 'ws';
import type { Owner, StateDocument } from '../../src/lib/contract/types';
import type {} from '../../src/lib/sync/client';

export const HEADERS = { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' };
export const REPLICAS = ['r1', 'r2', 'r3'] as const;
/** The run is against one faderz replica instead of the mock (playwright.config.ts). */
export const BACKEND = !!process.env['FADERZ_E2E_BACKEND'];

export interface AuditEntry {
  owner: Owner;
  commandId: string;
  type: 'set' | 'touch' | 'release';
  seq: number | null;
  path: string;
  value: unknown;
  revision: number;
  replica: string;
}

export interface CommandOutcome {
  commandId: string;
  owner: Owner | null;
  type: string;
  path: string | null;
  result: string;
  revision: number;
  replica: string;
}

export interface ConnStats {
  connection: string;
  handle: string | null;
  kind: string | null;
  mode: 'primary' | 'standby';
  replica: string;
  meterFramesSent: number;
  metersSkippedNoCredit: number;
  creditsOutstanding: number;
  messagesIn: number;
  messagesOut: number;
  openedAt: string;
}

export async function serverState(request: APIRequestContext): Promise<StateDocument> {
  return (await (await request.get('/api/v1/state')).json()) as StateDocument;
}

export async function fader(request: APIRequestContext, channel: string): Promise<number> {
  return (await serverState(request)).state.channels[channel]!.fader;
}

/**
 * One atomic PATCH of absolute values, for restoring state in a finally block,
 * where a failure would leak into later tests. A write the replica could not order in
 * time (503) and a control another session still holds (409, whose lease runs
 * out) are both transient on a loaded runner. Each attempt carries a new
 * command id: the replica refuses a resent one once it is too old, and
 * writing the same values twice changes nothing. Anything else fails at once.
 */
export async function patchAll(request: APIRequestContext, changes: Array<{ path: string; value: unknown }>): Promise<void> {
  const deadline = Date.now() + 20_000;
  for (let wait = 100; ; wait = Math.min(wait * 2, 1000)) {
    const res = await request.patch('/api/v1/state', { headers: HEADERS, data: { commandId: crypto.randomUUID(), changes } });
    if (res.ok()) return;
    const body = await res.text();
    if (![409, 503].includes(res.status()) || Date.now() > deadline) {
      throw new Error(`PATCH /state: ${res.status()} ${body}`);
    }
    console.warn(`patchAll: retrying after ${res.status()} ${body}`);
    await new Promise((resolve) => setTimeout(resolve, wait));
  }
}

/** One strict PATCH: a refusal or timeout fails the test, so a held control or slow ordering shows. */
export async function patch(request: APIRequestContext, path: string, value: unknown): Promise<void> {
  const res = await request.patch('/api/v1/state', {
    headers: HEADERS,
    data: { commandId: crypto.randomUUID(), changes: [{ path, value }] },
  });
  expect(res.ok()).toBe(true);
}

export async function audit(request: APIRequestContext): Promise<{ entries: AuditEntry[]; summary: { total: number; duplicates: number } }> {
  return (await (await request.get('/__mock/audit')).json()) as { entries: AuditEntry[]; summary: { total: number; duplicates: number } };
}

export async function commands(request: APIRequestContext): Promise<CommandOutcome[]> {
  return (await (await request.get('/__mock/commands')).json()) as CommandOutcome[];
}

export async function stats(request: APIRequestContext): Promise<ConnStats[]> {
  return ((await (await request.get('/__mock/stats')).json()) as { connections: ConnStats[] }).connections;
}

/** Every replica serving, both command logs and the counters empty; against the backend, no control owned. */
export async function resetMock(request: APIRequestContext): Promise<void> {
  if (BACKEND) {
    // A touch outlives a closed client by its lease plus the server's
    // liveness bound, up to 9 s: the next test starts once none is left.
    await expect.poll(async () => Object.keys((await serverState(request)).owners), { timeout: 15_000 }).toEqual([]);
    return;
  }
  expect((await request.post('/__mock/slow-link?ms=0')).ok()).toBe(true);
  for (const id of REPLICAS) expect((await request.post(`/__mock/replicas/${id}/restore`)).ok()).toBe(true);
  expect((await request.post('/__mock/blocks/flush')).ok()).toBe(true);
  await request.post('/__mock/audit/reset');
  await request.post('/__mock/stats/reset');
}

export function formatDb(db: number): string {
  return db > 0 ? `+${db.toFixed(1)}` : db.toFixed(1);
}

/** Loads the UI and waits for a live primary and, with replicas to stand by on, a ready standby. */
export async function live(page: Page, path = '/'): Promise<void> {
  await page.goto(path);
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  if (!BACKEND) await standbyReady(page);
}

export async function standbyReady(page: Page): Promise<void> {
  await page.waitForFunction(() => window.__faderz?.standby === 'Ready' && window.__faderz.connection === 'Live');
}

/** Every write the UI made has been answered. */
export async function settled(page: Page): Promise<void> {
  await expect(page.getByTestId('pending')).toHaveCount(0);
  await page.waitForFunction(() => window.__faderz?.outbox === 0);
}

export async function debug<K extends keyof NonNullable<Window['__faderz']>>(page: Page, key: K): Promise<NonNullable<Window['__faderz']>[K]> {
  return (await page.evaluate((k) => window.__faderz![k], key)) as NonNullable<Window['__faderz']>[K];
}

export async function faderCenter(page: Page, path: string): Promise<{ x: number; y: number }> {
  const el = page.locator(`[role="slider"][data-path="${path}"]`);
  await el.scrollIntoViewIfNeeded();
  const box = (await el.boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

export async function dragFader(page: Page, path: string, dy: number): Promise<void> {
  const { x, y } = await faderCenter(page, path);
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (let i = 1; i <= 5; i++) await page.mouse.move(x, y + (dy * i) / 5);
  await page.mouse.up();
}

type Message = { type: string; [k: string]: unknown };

/** A client on its own WebSocket: hello, heartbeat answers and commands. */
export class RawClient {
  readonly messages: Message[] = [];
  private readonly ws: WebSocket;
  private seq = Date.now();

  private constructor(url: string, readonly kind: 'ui' | 'console' | 'script', readonly session: string) {
    this.ws = new WebSocket(url);
    this.ws.on('message', (data, binary) => {
      if (binary) return;
      const msg = JSON.parse(String(data)) as Message;
      this.messages.push(msg);
      if (msg.type === 'ping') this.ws.send(JSON.stringify({ type: 'pong', t: msg['t'] }));
    });
  }

  /** Opens a primary connection on the service endpoint and waits for its snapshot. */
  static async open(baseURL: string, kind: 'ui' | 'console' | 'script' = 'ui', session = `raw-${crypto.randomUUID()}`): Promise<RawClient> {
    const c = new RawClient(`${baseURL.replace(/^http/, 'ws')}/api/v1/ws?session=${session}`, kind, session);
    await new Promise<void>((resolve, reject) => {
      c.ws.once('open', () => resolve());
      c.ws.once('error', reject);
    });
    c.ws.send(JSON.stringify({ type: 'hello', session: c.session, kind, mode: 'primary' }));
    await expect.poll(() => c.messages.some((m) => m.type === 'snapshot' || m.type === 'resume')).toBe(true);
    return c;
  }

  /** Sends a command and resolves with what answered it: an ack, an error, or the delta listing it. */
  async command(cmd: { type: 'set'; path: string; value: unknown } | { type: 'touch' | 'release'; path: string }): Promise<Message> {
    const id = crypto.randomUUID();
    const msg = { ...cmd, id, seq: ++this.seq };
    this.ws.send(JSON.stringify(msg));
    let answer: Message | undefined;
    await expect
      .poll(() => {
        answer = this.messages.find(
          (m) =>
            ((m.type === 'ack' || m.type === 'error') && m['commandId'] === id) ||
            (m.type === 'delta' && ((m['origins'] as Array<{ commandId: string }> | undefined) ?? []).some((o) => o.commandId === id)),
        );
        return answer !== undefined;
      })
      .toBe(true);
    return answer!;
  }

  /** Sends a message that is not a command, such as select or request. */
  send(msg: Message): void {
    this.ws.send(JSON.stringify(msg));
  }

  close(): void {
    this.ws.close();
  }
}

/** The UI face is loaded: the dev server served the @fontsource files. */
export async function uiFontLoaded(page: Page): Promise<boolean> {
  return page.evaluate(async () => {
    await document.fonts.ready;
    return [...document.fonts].some((f) => f.family.replace(/["']/g, '') === 'IBM Plex Sans Condensed' && f.status === 'loaded');
  });
}
