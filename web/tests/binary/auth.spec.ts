// The faderz binary serving the UI and the API in each auth mode.

import { expect, test, type Page } from '@playwright/test';
import WebSocket from 'ws';
import { PORTS, TOKEN } from '../../playwright.binary.config';

const WRITE = { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' };

/** In token mode the browser exchanges the token for a session cookie first. */
async function signIn(page: Page, mode: string): Promise<void> {
  if (mode !== 'token') return;
  const res = await page.request.post('/api/v1/login', { headers: WRITE, data: { token: TOKEN } });
  expect(res.status()).toBe(204);
}

/** The status a WebSocket upgrade gets, or 101. */
function upgrade(url: string, options: WebSocket.ClientOptions): Promise<{ status: number; socket?: WebSocket }> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, options);
    ws.once('open', () => resolve({ status: 101, socket: ws }));
    ws.once('unexpected-response', (_req, res) => resolve({ status: res.statusCode ?? 0 }));
    ws.once('error', reject);
  });
}

const session = () => `binary-${crypto.randomUUID()}`;

test('serves the UI, which goes live and writes through the API', { tag: '@all' }, async ({ page }, info) => {
  await signIn(page, info.project.name);
  await page.goto('/');
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  await page.getByRole('button', { name: 'Pres 1 mute' }).click();
  await expect
    .poll(async () => ((await (await page.request.get('/api/v1/state')).json()) as { state: { channels: { ch1: { mute: boolean } } } }).state.channels.ch1.mute)
    .toBe(true);
});

test('refuses a state-changing request without X-Samplez-Request', { tag: '@all' }, async ({ page }, info) => {
  await signIn(page, info.project.name);
  const res = await page.request.patch('/api/v1/state', {
    headers: { 'Content-Type': 'application/json' },
    data: { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch2/mute', value: true }] },
  });
  expect(res.status()).toBe(403);
});

test('answers a request with a foreign Host with 403', { tag: '@none' }, async ({ request }) => {
  expect((await request.get('/api/v1/state', { headers: { Host: 'rebound.example' } })).status()).toBe(403);
  expect((await request.get('/', { headers: { Host: 'rebound.example' } })).status()).toBe(403);
});

test('refuses a WebSocket upgrade from a foreign origin', { tag: '@none' }, async () => {
  const s = session();
  const url = `ws://127.0.0.1:${PORTS.none}/api/v1/ws?session=${s}`;
  expect((await upgrade(url, { origin: 'http://elsewhere.example' })).status).toBe(403);
  const own = await upgrade(url, { origin: `http://127.0.0.1:${PORTS.none}` });
  expect(own.status).toBe(101);
  own.socket!.close();
});

test('wants the token or its session cookie', { tag: '@token' }, async ({ request }) => {
  expect((await request.get('/api/v1/state')).status()).toBe(401);
  const junk = { Authorization: 'Bearer not-the-token' };
  expect((await request.get('/api/v1/state', { headers: junk })).status()).toBe(401);
  // Identity headers count for nothing outside proxy mode.
  expect((await request.get('/api/v1/state', { headers: { 'X-Forwarded-User': 'alice' } })).status()).toBe(401);
  const bearer = { Authorization: `Bearer ${TOKEN}` };
  expect((await request.get('/api/v1/state', { headers: bearer })).status()).toBe(200);
});

test('takes who the proxy names, not identity headers a client sends', { tag: '@proxy' }, async ({ baseURL }) => {
  const s = session();
  const url = `${baseURL!.replace(/^http/, 'ws')}/api/v1/ws?session=${s}`;
  const { socket } = await upgrade(url, { origin: baseURL!, headers: { 'X-Forwarded-User': 'mallory', 'X-Forwarded-Groups': 'operators' } });
  const snapshot = await new Promise<Record<string, unknown>>((resolve) => {
    socket!.on('message', (data) => {
      const m = JSON.parse(String(data)) as Record<string, unknown>;
      if (m['type'] === 'snapshot') resolve(m);
    });
    socket!.send(JSON.stringify({ type: 'hello', session: s, kind: 'ui', mode: 'primary' }));
  });
  expect((snapshot['you'] as { principal: string }).principal).toBe('alice');
  socket!.close();
});
