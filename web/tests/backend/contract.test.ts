// The faderz backend against the committed contract: every REST response
// against openapi.yaml, every WebSocket message against
// ws/server-message.schema.json, with the validators the mock uses.

import { spawn, type ChildProcess } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import WebSocket from 'ws';
import { Contract } from '../../dev/mock/contract';
import { exampleShape } from '../../dev/mock/shape';

const BIN = process.env['FADERZ_BIN'];
const PORT = Number(process.env['FADERZ_BACKEND_PORT'] ?? 6461);
const BASE = `http://127.0.0.1:${PORT}/api/v1`;
const DIR = resolve('node_modules/.cache/faderz-backend-contract');
const HEADERS = { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' };
const contract = new Contract();
let server: ChildProcess;

beforeAll(async () => {
  if (!BIN) throw new Error('FADERZ_BIN names the faderz binary under test');
  rmSync(DIR, { recursive: true, force: true });
  mkdirSync(DIR, { recursive: true });
  const listener = (port: number) => ({ bind: '127.0.0.1', port });
  const config = {
    listeners: { api: listener(PORT), ops: listener(PORT + 1), nmos: listener(PORT + 2) },
    replica: { name: 'r1', dataDir: `${DIR}/data` },
    shape: exampleShape(),
  };
  writeFileSync(`${DIR}/config.json`, JSON.stringify(config));
  server = spawn(resolve(BIN), ['--config', `${DIR}/config.json`], { stdio: ['ignore', 'ignore', 'inherit'] });
  for (let i = 0; ; i++) {
    const up = await fetch(`${BASE}/replicas`).then((r) => r.ok, () => false);
    if (up) break;
    if (i > 100) throw new Error('the backend did not come up');
    await new Promise((r) => setTimeout(r, 100));
  }
});

afterAll(() => {
  server?.kill('SIGTERM');
});

/** Sends a request and checks the answer against the operation in openapi.yaml. */
async function call(method: string, path: string, template: string, body?: unknown, headers = HEADERS): Promise<{ status: number; body: unknown }> {
  const res = await fetch(BASE + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  const parsed = text ? (JSON.parse(text) as unknown) : undefined;
  contract.assertResponse(template, method, res.status, parsed);
  return { status: res.status, body: parsed };
}

const id = () => crypto.randomUUID();

describe('REST', () => {
  it('reads and writes the state', async () => {
    expect((await call('GET', '/state', '/state')).status).toBe(200);
    expect((await call('GET', '/replicas', '/replicas')).status).toBe(200);
    const write = { commandId: id(), changes: [{ path: '/channels/ch1/fader', value: -3 }] };
    expect((await call('PATCH', '/state', '/state', write)).body).toMatchObject({ result: 'applied' });
    expect((await call('PATCH', '/state', '/state', write)).body).toMatchObject({ result: 'duplicate' });
    const same = { commandId: id(), changes: write.changes };
    expect((await call('PATCH', '/state', '/state', same)).body).toMatchObject({ result: 'noop' });
    const unknown = { commandId: id(), changes: [{ path: '/channels/zz/fader', value: 0 }] };
    expect((await call('PATCH', '/state', '/state', unknown)).status).toBe(404);
    const invalid = { commandId: id(), changes: [{ path: '/channels/ch1/fader', value: 99 }] };
    expect((await call('PATCH', '/state', '/state', invalid)).status).toBe(400);
  });

  it('refuses a write to an owned control with its value and owner', async () => {
    const session = `contract-${id()}`;
    const ws = new WebSocket(`ws://127.0.0.1:${PORT}/api/v1/ws?session=${session}`);
    const seen: Array<Record<string, unknown>> = [];
    ws.on('message', (data, binary) => {
      if (binary) return;
      const m = JSON.parse(String(data)) as Record<string, unknown>;
      contract.assertServerMessage(m);
      seen.push(m);
      if (m['type'] === 'ping') ws.send(JSON.stringify({ type: 'pong', t: m['t'] }));
    });
    await new Promise((r) => ws.once('open', r));
    ws.send(JSON.stringify({ type: 'hello', session, kind: 'ui', mode: 'primary' }));
    await expect.poll(() => seen.some((m) => m['type'] === 'snapshot')).toBe(true);
    const touch = id();
    ws.send(JSON.stringify({ type: 'touch', id: touch, seq: 1, path: '/channels/ch2/fader' }));
    await expect.poll(() => seen.some((m) => JSON.stringify(m['origins'] ?? []).includes(touch))).toBe(true);
    const write = { commandId: id(), changes: [{ path: '/channels/ch2/fader', value: 0 }] };
    const refused = await call('PATCH', '/state', '/state', write);
    expect(refused.status).toBe(409);
    expect(refused.body).toMatchObject({ conflicts: [{ path: '/channels/ch2/fader' }] });
    ws.close();
  });

  it('stores, renames, recalls and deletes scenes', async () => {
    const created = await call('POST', '/scenes', '/scenes', { commandId: id(), name: 'Opening' });
    expect(created.status).toBe(201);
    const scene = (created.body as { scene: { id: string } }).scene.id;
    const stored = await call('PUT', `/scenes/${scene}`, '/scenes/{sceneId}', { commandId: id(), name: 'Open' });
    expect(stored.body).toMatchObject({ scene: { id: scene, name: 'Open' } });
    const recall = await call('POST', `/scenes/${scene}/recall`, '/scenes/{sceneId}/recall', { commandId: id(), crossfadeMs: 0 });
    expect(recall.body).toMatchObject({ result: 'applied' });
    expect((await call('DELETE', `/scenes/${scene}`, '/scenes/{sceneId}')).status).toBe(200);
    expect((await call('DELETE', `/scenes/${scene}`, '/scenes/{sceneId}')).status).toBe(404);
    expect((await call('PUT', `/scenes/${scene}`, '/scenes/{sceneId}', { commandId: id() })).status).toBe(404);
  });

  it('takes tallies and loudness resets', async () => {
    expect((await call('POST', '/afv/tally', '/afv/tally', { source: 'CAM1', state: 'program' })).body).toMatchObject({ result: 'applied' });
    expect((await call('POST', '/afv/tally', '/afv/tally', { source: 'CAM1', state: 'program' })).body).toMatchObject({ result: 'noop' });
    expect((await call('PUT', '/afv/tally', '/afv/tally', { sources: { CAM2: 'preview' } })).status).toBe(200);
    expect((await call('POST', '/loudness/reset', '/loudness/reset', { ids: ['pgm20'] })).status).toBe(204);
    expect((await call('POST', '/loudness/reset', '/loudness/reset', { ids: ['ch1'] })).status).toBe(404);
  });

  it('creates, lists and removes consoles, and takes a registration over the WebSocket', async () => {
    const path = '/consoles/desk-1';
    const created = await call('PUT', path, '/consoles/{consoleId}', { name: 'Desk' });
    expect(created).toMatchObject({ status: 201, body: { id: 'desk-1', handle: null, connected: false, strips: 8 } });
    const updated = await call('PUT', path, '/consoles/{consoleId}', { name: 'Desk', strips: 16 });
    expect(updated).toMatchObject({ status: 200, body: { strips: 16 } });
    expect((await call('GET', '/consoles', '/consoles')).body).toContainEqual(updated.body);

    const session = `contract-${id()}`;
    const ws = new WebSocket(`ws://127.0.0.1:${PORT}/api/v1/ws?session=${session}`);
    const seen: Array<Record<string, unknown>> = [];
    ws.on('message', (data, binary) => {
      if (binary) return;
      const m = JSON.parse(String(data)) as Record<string, unknown>;
      contract.assertServerMessage(m);
      seen.push(m);
      if (m['type'] === 'ping') ws.send(JSON.stringify({ type: 'pong', t: m['t'] }));
    });
    await new Promise((r) => ws.once('open', r));
    ws.send(JSON.stringify({ type: 'hello', session, kind: 'console', mode: 'primary' }));
    await expect.poll(() => seen.some((m) => m['type'] === 'snapshot')).toBe(true);
    const registration = { type: 'console', id: 'desk-1', name: 'Desk', protocol: 'mcu', transport: 'bridge', strips: 16, bankOffset: 8 };
    expect(contract.checkClientMessage(registration)).toBeNull();
    ws.send(JSON.stringify(registration));
    const registered = () =>
      seen.some((m) => JSON.stringify(m['changes'] ?? []).includes('"bankOffset":8') && JSON.stringify(m['changes']).includes('"connected":true'));
    await expect.poll(registered).toBe(true);
    // Connected, it is not removed; once the session's lease has expired it is.
    expect(await call('DELETE', path, '/consoles/{consoleId}')).toMatchObject({ status: 409, body: { message: 'console desk-1 is connected' } });
    ws.close();
    await expect.poll(async () => (await call('DELETE', path, '/consoles/{consoleId}')).status, { timeout: 15000, interval: 500 }).toBe(204);
    expect((await call('DELETE', path, '/consoles/{consoleId}')).status).toBe(404);
  });

  it('refuses a state-changing request without X-Samplez-Request', async () => {
    const res = await fetch(`${BASE}/state`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    // samplez-service answers 403 before faderz sees the request, with no
    // body where openapi.yaml declares an Error.
    expect(res.status).toBe(403);
  });
});
