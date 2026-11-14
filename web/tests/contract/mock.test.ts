// Drives the dev mock over real HTTP and WebSocket connections and checks
// every message and response against the committed contract with an
// independent validator.

import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import WebSocket from 'ws';
import { Cluster } from '../../dev/mock/cluster';
import { Contract } from '../../dev/mock/contract';
import { MockServer } from '../../dev/mock/server';
import type { ServerMessage, SnapshotMessage } from '../../src/lib/contract/types';

const contract = new Contract();
let server: Server;
let cluster: Cluster;
let base: string;

beforeAll(async () => {
  cluster = new Cluster({ automation: false, replicas: 3 });
  const mock = new MockServer(cluster);
  server = createServer((req, res) =>
    mock.middleware(req, res, () => {
      res.statusCode = 404;
      res.end();
    }),
  );
  mock.attach(server);
  cluster.start();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  base = `127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(async () => {
  cluster.stop();
  await new Promise((r) => server.close(r));
});

class Client {
  readonly messages: ServerMessage[] = [];
  private waiters: Array<() => void> = [];
  ws!: WebSocket;
  readonly clientId: string;
  private seq = 0;

  constructor(clientId: string = crypto.randomUUID()) {
    this.clientId = clientId;
  }

  async open(path = '/api/v1/ws'): Promise<this> {
    this.ws = new WebSocket(`ws://${base}${path}`);
    this.ws.on('message', (data) => {
      const msg = JSON.parse(String(data)) as ServerMessage;
      if (!contract.serverMessage(msg)) {
        throw new Error(`invalid server message: ${JSON.stringify(contract.serverMessage.errors)}`);
      }
      this.messages.push(msg);
      for (const w of this.waiters.splice(0)) w();
    });
    await new Promise((r, j) => {
      this.ws.once('open', r);
      this.ws.once('error', j);
    });
    return this;
  }

  send(msg: object): void {
    expect(contract.checkClientMessage(msg)).toBeNull();
    this.ws.send(JSON.stringify(msg));
  }

  async hello(lastRevision?: number): Promise<ServerMessage> {
    const from = this.messages.length;
    this.send({ type: 'hello', clientId: this.clientId, ...(lastRevision === undefined ? {} : { lastRevision }) });
    return this.next((m) => m.type === 'snapshot' || m.type === 'resume', from);
  }

  set(path: string, value: unknown, id: string = crypto.randomUUID(), seq = ++this.seq) {
    this.send({ type: 'set', id, seq, path, value });
    return id;
  }

  command(type: 'touch' | 'release', path: string, id: string = crypto.randomUUID()) {
    this.send({ type, id, path });
    return id;
  }

  async next(pred: (m: ServerMessage) => boolean, from = 0, timeout = 3000): Promise<ServerMessage> {
    const deadline = Date.now() + timeout;
    for (;;) {
      const hit = this.messages.slice(from).find(pred);
      if (hit) return hit;
      if (Date.now() > deadline) throw new Error('timed out waiting for message');
      await new Promise<void>((r) => {
        this.waiters.push(r);
        setTimeout(r, 50);
      });
    }
  }

  answer(commandId: string, from = 0) {
    return this.next(
      (m) =>
        (m.type === 'delta' && m.origin?.commandId === commandId) ||
        ((m.type === 'ack' || m.type === 'error') && m.commandId === commandId),
      from,
    );
  }

  close(): Promise<void> {
    return new Promise((r) => {
      this.ws.once('close', () => r());
      this.ws.close();
    });
  }
}

async function rest(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await fetch(`http://${base}/api/v1${path}`, {
    method,
    headers: { 'X-Samplez-Request': '1', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  return { status: res.status, body: text ? (JSON.parse(text) as Record<string, unknown>) : undefined };
}

describe('WebSocket', () => {
  it('answers hello with replicas and a snapshot of the global revision', async () => {
    const c = await new Client().open();
    const snap = (await c.hello()) as SnapshotMessage;
    expect(snap.type).toBe('snapshot');
    expect(snap.revision).toBe(cluster.revision);
    expect(snap.clientId).toBe(c.clientId);
    expect(c.messages[0]!.type).toBe('replicas');
    expect(Object.keys(snap.state.channels)).toHaveLength(29);
    await c.close();
  });

  it('fans a set out as a delta with origin and atIndex to every replica', async () => {
    const a = await new Client().open('/__mock/replicas/r1/api/v1/ws');
    const b = await new Client().open('/__mock/replicas/r2/api/v1/ws');
    await a.hello();
    await b.hello();
    const id = a.set('/channels/ch3/fader', -7.5);
    const da = await a.answer(id);
    const db = await b.next((m) => m.type === 'delta' && m.origin?.commandId === id);
    expect(da).toMatchObject({ type: 'delta', origin: { clientId: a.clientId, commandId: id }, replica: { id: 'r1' } });
    expect(db).toMatchObject({ revision: (da as { revision: number }).revision, replica: { id: 'r2' } });
    expect((da as { atIndex?: number }).atIndex).toBeGreaterThan(0);
    await a.close();
    await b.close();
  });

  it('applies a command id once and drops older sequence numbers', async () => {
    const c = await new Client().open();
    await c.hello();
    const id = c.set('/channels/ch4/fader', -9, undefined, 10);
    await c.answer(id);
    const rev = cluster.revision;
    const from = c.messages.length;
    c.set('/channels/ch4/fader', -9, id, 10);
    expect(await c.answer(id, from)).toMatchObject({ type: 'ack', result: 'duplicate', revision: rev });
    const stale = c.set('/channels/ch4/fader', -20, undefined, 5);
    expect(await c.answer(stale)).toMatchObject({ type: 'ack', result: 'stale' });
    expect(cluster.state.channels['ch4']!.fader).toBe(-9);
    await c.close();
  });

  it('refuses writes to a control another client owns with 409 and the current value', async () => {
    const a = await new Client().open();
    const b = await new Client().open();
    await a.hello();
    await b.hello();
    const path = '/channels/ch5/fader';
    const touch = a.command('touch', path);
    expect(await a.answer(touch)).toMatchObject({ type: 'delta', owners: [{ path, owner: a.clientId }] });
    const w = b.set(path, 3);
    expect(await b.answer(w)).toMatchObject({ type: 'error', code: 409, path, owner: a.clientId, current: cluster.state.channels['ch5']!.fader });
    const t = b.command('touch', path);
    expect(await b.answer(t)).toMatchObject({ type: 'error', code: 409 });
    const rel = a.command('release', path);
    expect(await a.answer(rel)).toMatchObject({ type: 'delta', owners: [{ path, owner: null }] });
    await a.close();
    await b.close();
  });

  it('keeps ownership across a replica switch and releases it 2 s after the client is gone', async () => {
    const path = '/channels/ch6/fader';
    const a = await new Client().open('/__mock/replicas/r1/api/v1/ws');
    await a.hello();
    await a.answer(a.command('touch', path));
    await a.close();
    const again = await new Client(a.clientId).open('/__mock/replicas/r3/api/v1/ws');
    await again.hello(cluster.revision);
    await new Promise((r) => setTimeout(r, 2300));
    expect(cluster.owners[path]).toBe(a.clientId);
    await again.close();
    await new Promise((r) => setTimeout(r, 2300));
    expect(cluster.owners[path]).toBeUndefined();
  });

  it('resumes from the last seen revision with every missed delta in order', async () => {
    const a = await new Client().open();
    const snap = (await a.hello()) as SnapshotMessage;
    await a.close();
    const other = await new Client().open();
    await other.hello();
    for (const v of [-1, -2, -3]) await other.answer(other.set('/channels/ch7/fader', v));
    const back = await new Client(a.clientId).open('/__mock/replicas/r2/api/v1/ws');
    const resume = await back.hello(snap.revision);
    expect(resume.type).toBe('resume');
    const revs = (resume as { deltas: Array<{ revision: number }> }).deltas.map((d) => d.revision);
    expect(revs[0]).toBe(snap.revision + 1);
    expect(revs.every((r, i) => i === 0 || r === revs[i - 1]! + 1)).toBe(true);
    expect((resume as { revision: number }).revision).toBe(revs.at(-1));
    await other.close();
    await back.close();
  });

  it('falls back to a snapshot when the log no longer covers the gap', async () => {
    const c = await new Client().open();
    const snap = (await c.hello()) as SnapshotMessage;
    cluster.skipRevision();
    const again = await c.hello(snap.revision);
    expect(again).toMatchObject({ type: 'snapshot', revision: cluster.revision });
    await c.close();
  });

  it('streams meters and counters of the serving replica covering the whole shape', async () => {
    const c = await new Client().open('/__mock/replicas/r2/api/v1/ws');
    await c.hello();
    const meters = await c.next((m) => m.type === 'meters');
    const counters = await c.next((m) => m.type === 'counters', 0, 2500);
    expect(meters).toMatchObject({ replica: { id: 'r2' } });
    expect(counters).toMatchObject({ replica: { id: 'r2' } });
    const m = meters as { channels: object; buses: object; auxes: object; mixMinus: object };
    expect(Object.keys(m.channels)).toHaveLength(29);
    expect(Object.keys(m.buses)).toEqual(['pgm51', 'pgm20', 'me', 'cf']);
    expect(Object.keys(m.auxes)).toHaveLength(4);
    expect(Object.keys(m.mixMinus)).toHaveLength(2);
    await c.close();
  });

  it('answers an invalid message with error 400', async () => {
    const c = await new Client().open();
    await c.hello();
    c.ws.send(JSON.stringify({ type: 'set', id: crypto.randomUUID(), seq: 1, path: '/channels/ch1/fader', value: 99 }));
    expect(await c.next((m) => m.type === 'error')).toMatchObject({ code: 400 });
    await c.close();
  });

  it('closes clients of a killed replica, re-elects the leader and restores it', async () => {
    const a = await new Client().open('/__mock/replicas/r1/api/v1/ws');
    const b = await new Client().open('/__mock/replicas/r2/api/v1/ws');
    await a.hello();
    await b.hello();
    const closed = new Promise((r) => a.ws.once('close', r));
    cluster.kill('r1');
    await closed;
    const status = await b.next((m) => m.type === 'replicas' && m.replicas.some((r) => r.id === 'r1' && !r.healthy), 0);
    expect(status).toMatchObject({ replicas: expect.arrayContaining([expect.objectContaining({ id: 'r2', role: 'leader' })]) });
    cluster.restore('r1');
    await b.close();
  });

  it('applies commands sent to a replica that hangs after replicating them', async () => {
    const a = await new Client().open('/__mock/replicas/r3/api/v1/ws');
    const b = await new Client().open('/__mock/replicas/r2/api/v1/ws');
    await a.hello();
    await b.hello();
    cluster.freeze('r3', 'apply');
    const from = a.messages.length;
    const id = a.set('/channels/ch8/mute', true);
    await b.next((m) => m.type === 'delta' && m.origin?.commandId === id);
    await new Promise((r) => setTimeout(r, 200));
    expect(a.messages.slice(from)).toHaveLength(0);
    cluster.restore('r3');
    await b.close();
  });
});

describe('REST', () => {
  it('reads the state document', async () => {
    const res = await rest('GET', '/state');
    expect(res.status).toBe(200);
    expect(res.body).toMatchObject({ revision: cluster.revision });
  });

  it('patches atomically, once per command id', async () => {
    const commandId = crypto.randomUUID();
    const body = { commandId, changes: [{ path: '/channels/ch9/fader', value: -4 }, { path: '/channels/ch9/mute', value: true }] };
    const first = await rest('PATCH', '/state', body);
    expect(first).toMatchObject({ status: 200, body: { result: 'applied', commandId } });
    const second = await rest('PATCH', '/state', body);
    expect(second).toMatchObject({ status: 200, body: { result: 'duplicate', revision: first.body!['revision'] } });
  });

  it('refuses owned controls with 409 and the current value', async () => {
    const c = await new Client().open();
    await c.hello();
    await c.answer(c.command('touch', '/channels/ch10/fader'));
    const res = await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch10/fader', value: 0 }] });
    expect(res).toMatchObject({ status: 409, body: { conflicts: [{ path: '/channels/ch10/fader', owner: c.clientId, current: -90 }] } });
    await c.close();
  });

  it('refuses invalid bodies, unknown controls and requests without the CSRF header', async () => {
    expect((await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch1/fader', value: 20 }] })).status).toBe(400);
    expect((await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch99/fader', value: 0 }] })).status).toBe(404);
    const res = await fetch(`http://${base}/api/v1/state`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: '{}' });
    expect(res.status).toBe(403);
  });

  it('stores, recalls with recall-safe and deletes scenes', async () => {
    const created = await rest('POST', '/scenes', { commandId: crypto.randomUUID(), name: 'Test scene' });
    expect(created.status).toBe(201);
    const id = (created.body!['scene'] as { id: string }).id;
    await rest('PATCH', '/state', {
      commandId: crypto.randomUUID(),
      changes: [{ path: '/channels/ch2/fader', value: -30 }, { path: '/channels/ch11/fader', value: -30 }],
    });
    const recall = await rest('POST', `/scenes/${id}/recall`, { commandId: crypto.randomUUID(), crossfadeMs: 500 });
    expect(recall).toMatchObject({ status: 200, body: { result: 'applied' } });
    expect(cluster.state.channels['ch2']!.fader).not.toBe(-30);
    expect(cluster.state.channels['ch11']!.fader).toBe(-30);
    expect((await rest('PUT', `/scenes/${id}`, { commandId: crypto.randomUUID(), name: 'Renamed' })).status).toBe(200);
    expect((await rest('DELETE', `/scenes/${id}`)).status).toBe(200);
    expect((await rest('POST', `/scenes/${id}/recall`, { commandId: crypto.randomUUID(), crossfadeMs: 0 })).status).toBe(404);
  });

  it('drives AFV stages from tally', async () => {
    await rest('PUT', '/afv/tally', { sources: { CAM1: 'off', CAM2: 'program' } });
    expect(cluster.state.afv['ch14']!.open).toBe(true);
    expect(cluster.state.afv['ch13']!.open).toBe(false);
    const res = await rest('POST', '/afv/tally', { source: 'CAM1', state: 'program', transitionMs: 40 });
    expect(res).toMatchObject({ status: 200, body: { result: 'applied' } });
    expect(cluster.state.afv['ch13']!.open).toBe(true);
  });

  it('resets loudness and lists replicas', async () => {
    expect((await rest('POST', '/loudness/reset', { ids: ['pgm51'] })).status).toBe(204);
    expect((await rest('POST', '/loudness/reset', { ids: ['nope'] })).status).toBe(404);
    expect((await rest('GET', '/replicas')).body!['replicas']).toHaveLength(3);
  });
});
