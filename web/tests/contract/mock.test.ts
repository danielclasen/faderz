// Drives the dev mock over real HTTP and WebSocket connections and checks
// every message and response against the committed contract with an
// independent validator: JSON messages against the message schemas,
// binary meter frames by decoding them against the layout of the
// snapshot's shape and validating the decoded meters.

import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import WebSocket from 'ws';
import { Cluster, type ClusterOptions } from '../../dev/mock/cluster';
import { Contract } from '../../dev/mock/contract';
import { MockServer } from '../../dev/mock/server';
import { DEFAULT_CHANNELS, exampleShape } from '../../dev/mock/shape';
import { decodeMeterFrame, deriveMeterLayout, type MeterFrame, type MeterLayout } from '../../src/lib/contract/meterFrame';
import { DEAD_MS, HEARTBEAT_MS, LEASE_MS, METER_CREDIT_CAP, METER_PERIOD_MS } from '../../src/lib/contract/protocol';
import type {
  AckMessage,
  DeltaMessage,
  ErrorMessage,
  LinkMode,
  Owner,
  ReplicasMessage,
  ServerMessage,
  SnapshotMessage,
} from '../../src/lib/contract/types';

const contract = new Contract();
/** Contract violations seen by any client; checked after every test. */
const failures: string[] = [];
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

interface Mock {
  cluster: Cluster;
  base: string;
  stop(): Promise<void>;
}

async function startMock(options: ClusterOptions): Promise<Mock> {
  const cluster = new Cluster(options);
  const mock = new MockServer(cluster);
  const server: Server = createServer((req, res) =>
    mock.middleware(req, res, () => {
      res.statusCode = 404;
      res.end();
    }),
  );
  mock.attach(server);
  cluster.start();
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  return {
    cluster,
    base: `127.0.0.1:${(server.address() as AddressInfo).port}`,
    stop: async () => {
      cluster.stop();
      await new Promise((r) => server.close(r));
    },
  };
}

let main: Mock;
let cluster: Cluster;
const clients: Client[] = [];

beforeAll(async () => {
  main = await startMock({ automation: false, replicas: 3 });
  cluster = main.cluster;
});

afterAll(() => main.stop());

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close()));
  cluster.holdBlock(false);
  for (const id of ['r1', 'r2', 'r3']) await control('POST', `/replicas/${id}/restore`);
  expect(failures.splice(0)).toEqual([]);
});

const randomSession = () => `s-${crypto.randomUUID()}`;

class Client {
  readonly messages: ServerMessage[] = [];
  readonly frames: MeterFrame[] = [];
  readonly session: string;
  readonly kind: 'ui' | 'console';
  ws!: WebSocket;
  you: Owner | null = null;
  /** Replica id of the snapshot or resume. */
  replica: string | null = null;
  layout: MeterLayout | null = null;
  /** Answer server pings with pong. */
  autoPong = true;
  /** Send a ping every HEARTBEAT_MS after hello, as the web client does. */
  heartbeat = true;
  closed!: Promise<void>;
  private waiters: Array<() => void> = [];
  private seq = Date.now();
  private pinger: ReturnType<typeof setInterval> | null = null;

  constructor(session: string = randomSession(), kind: 'ui' | 'console' = 'ui') {
    this.session = session;
    this.kind = kind;
    clients.push(this);
  }

  async open(path = '/api/v1/ws', base = main.base): Promise<this> {
    this.ws = new WebSocket(`ws://${base}${path}${path.includes('?') ? '&' : '?'}session=${encodeURIComponent(this.session)}`);
    this.closed = new Promise((r) => this.ws.once('close', () => r()));
    this.ws.on('message', (data, isBinary) => (isBinary ? this.binary(data as Buffer) : this.text(String(data))));
    await new Promise((r, j) => {
      this.ws.once('open', r);
      this.ws.once('error', j);
    });
    void this.closed.then(() => this.stopHeartbeat());
    return this;
  }

  private text(raw: string): void {
    const msg = JSON.parse(raw) as ServerMessage;
    if (!contract.serverMessage(msg)) failures.push(`invalid server message ${raw}: ${JSON.stringify(contract.serverMessage.errors)}`);
    if (msg.type === 'snapshot') this.layout = deriveMeterLayout(msg.shape);
    if (msg.type === 'snapshot' || msg.type === 'resume') {
      this.you = msg.you;
      this.replica = msg.replica.id;
    }
    if (msg.type === 'ping' && this.autoPong) this.send({ type: 'pong', t: msg.t });
    this.push(msg);
  }

  private binary(data: Buffer): void {
    if (!this.layout) {
      failures.push('meter frame before a snapshot');
      return;
    }
    try {
      const frame = decodeMeterFrame(this.layout, data);
      contract.assertMeterValues(frame);
      this.frames.push(frame);
    } catch (e) {
      failures.push(`meter frame: ${String(e)}`);
    }
    for (const w of this.waiters.splice(0)) w();
  }

  private push(msg: ServerMessage): void {
    this.messages.push(msg);
    for (const w of this.waiters.splice(0)) w();
  }

  send(msg: object): void {
    expect(contract.checkClientMessage(msg)).toBeNull();
    if (this.ws.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  async hello(mode: LinkMode = 'primary', lastRevision?: number): Promise<ServerMessage> {
    const from = this.messages.length;
    this.send({ type: 'hello', session: this.session, kind: this.kind, mode, ...(lastRevision === undefined ? {} : { lastRevision }) });
    const answer = await this.next((m) => m.type === 'snapshot' || m.type === 'resume', from);
    if (this.heartbeat) this.pinger = setInterval(() => this.send({ type: 'ping', t: Date.now() }), HEARTBEAT_MS);
    return answer;
  }

  set(path: string, value: unknown, id: string = crypto.randomUUID(), seq = ++this.seq): string {
    this.send({ type: 'set', id, seq, path, value });
    return id;
  }

  command(type: 'touch' | 'release', path: string, id: string = crypto.randomUUID()): string {
    this.send({ type, id, path });
    return id;
  }

  credit(n: number): void {
    this.send({ type: 'credit', n });
  }

  async next<T extends ServerMessage = ServerMessage>(pred: (m: ServerMessage) => boolean, from = 0, timeout = 3000): Promise<T> {
    const deadline = Date.now() + timeout;
    for (;;) {
      const hit = this.messages.slice(from).find(pred);
      if (hit) return hit as T;
      if (Date.now() > deadline) throw new Error('timed out waiting for message');
      await new Promise<void>((r) => {
        this.waiters.push(r);
        setTimeout(r, 50);
      });
    }
  }

  /** The delta listing the command among its origins, or its ack or error. */
  answer<T extends ServerMessage = ServerMessage>(commandId: string, from = 0): Promise<T> {
    return this.next<T>((m) => answers(m, commandId), from);
  }

  count(type: ServerMessage['type'], from = 0): number {
    return this.messages.slice(from).filter((m) => m.type === type).length;
  }

  private stopHeartbeat(): void {
    if (this.pinger) clearInterval(this.pinger);
    this.pinger = null;
  }

  close(): Promise<void> {
    this.stopHeartbeat();
    if (!this.ws || this.ws.readyState === WebSocket.CLOSED) return Promise.resolve();
    this.ws.close();
    return this.closed;
  }
}

function answers(m: ServerMessage, commandId: string): boolean {
  return (
    (m.type === 'delta' && (m.origins ?? []).some((o) => o.commandId === commandId)) ||
    ((m.type === 'ack' || m.type === 'error') && m.commandId === commandId)
  );
}

const onReplica = (id: string) => `/__mock/replicas/${id}/api/v1/ws`;

async function rest(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const res = await fetch(`http://${main.base}/api/v1${path}`, {
    method,
    headers: { 'X-Samplez-Request': '1', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  return { status: res.status, body: text ? (JSON.parse(text) as Record<string, unknown>) : undefined };
}

async function control<T = Record<string, unknown>>(method: 'GET' | 'POST', path: string, base = main.base): Promise<{ status: number; body: T }> {
  const res = await fetch(`http://${base}/__mock${path}`, { method });
  return { status: res.status, body: (await res.json()) as T };
}

async function owners(): Promise<Record<string, Owner>> {
  return (await rest('GET', '/state')).body!['owners'] as Record<string, Owner>;
}

interface StatsRow {
  connection: string;
  session: string | null;
  kind: string | null;
  mode: LinkMode;
  replica: string;
  meterFramesSent: number;
  metersSkippedNoCredit: number;
  creditsOutstanding: number;
  messagesIn: number;
  messagesOut: number;
  openedAt: string;
}
interface Stats {
  connections: StatsRow[];
  totals: { connections: number; meterFramesSent: number; metersSkippedNoCredit: number; messagesIn: number; messagesOut: number };
}
interface Audit {
  entries: Array<{ owner: Owner; commandId: string; seq: number | null; path: string; value: unknown; revision: number; replica: string }>;
  summary: { total: number; duplicates: number };
}

const statsOf = async (c: Client) => (await control<Stats>('GET', '/stats')).body.connections.find((r) => r.connection === c.you!.connection)!;

describe('hello', () => {
  it('answers a primary hello with replicas and a snapshot carrying the owner record', async () => {
    const c = await new Client().open();
    const snap = (await c.hello()) as SnapshotMessage;
    expect(c.messages[0]!.type).toBe('replicas');
    expect(snap).toMatchObject({ type: 'snapshot', revision: cluster.revision });
    expect(snap.you).toEqual({ principal: 'anonymous', session: c.session, kind: 'ui', connection: expect.any(String) });
    expect(snap.state.consoles.every((k) => typeof k.session === 'string')).toBe(true);
  });

  it('serves a standby deltas, replicas and pings, but no meter frames or counters', async () => {
    const standby = await new Client().open(onReplica('r2'));
    await standby.hello('standby');
    standby.credit(METER_CREDIT_CAP);
    const primary = await new Client().open(onReplica('r2'));
    await primary.hello('primary');
    const id = primary.set('/channels/ch3/trim', 2);
    await standby.answer(id);
    await sleep(1100);
    expect(standby.count('ping')).toBeGreaterThanOrEqual(3);
    expect(standby.count('replicas')).toBeGreaterThanOrEqual(1);
    expect(standby.count('counters')).toBe(0);
    expect(standby.frames).toHaveLength(0);
    expect(primary.count('counters')).toBeGreaterThanOrEqual(1);
    expect((await statsOf(standby)).metersSkippedNoCredit).toBe(0);
  });

  it('promotes a standby to primary, which then spends the credits granted while standby', async () => {
    const c = await new Client().open(onReplica('r3'));
    await c.hello('standby');
    c.credit(5);
    await sleep(300);
    expect(c.frames).toHaveLength(0);
    expect((await statsOf(c)).creditsOutstanding).toBe(5);
    c.send({ type: 'promote', lastRevision: cluster.revision });
    await sleep(500);
    expect(c.frames).toHaveLength(5);
    await c.next((m) => m.type === 'counters', 0, 1500);
    const row = await statsOf(c);
    expect(row).toMatchObject({ mode: 'primary', creditsOutstanding: 0, meterFramesSent: 5 });
  });

  it('is answered as a console client with kind console', async () => {
    const session = cluster.state.consoles[0]!.session;
    const c = await new Client(session, 'console').open();
    const snap = (await c.hello()) as SnapshotMessage;
    expect(snap.you).toMatchObject({ session, kind: 'console' });
    const id = c.command('touch', '/channels/ch24/fader');
    const delta = await c.answer<DeltaMessage>(id);
    expect(delta.owners).toEqual([{ path: '/channels/ch24/fader', owner: snap.you }]);
    await c.answer(c.command('release', '/channels/ch24/fader'));
  });
});

describe('commands', () => {
  it('fans a set out as a delta whose origins carry the owner record, command id and seq', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const b = await new Client().open(onReplica('r2'));
    await b.hello();
    const id = a.set('/channels/ch2/fader', -7, undefined, 4242);
    const delta = await a.answer<DeltaMessage>(id);
    expect(delta.origins).toEqual([{ owner: a.you, commandId: id, seq: 4242 }]);
    expect(delta.changes).toEqual([{ path: '/channels/ch2/fader', value: -7 }]);
    expect(delta.atIndex).toBeGreaterThan(0);
    const seen = await b.answer<DeltaMessage>(id);
    expect(seen.revision).toBe(delta.revision);
    expect(seen.replica.id).toBe('r2');
  });

  it('batches the sets of two sessions in one block into one delta; the later write to a control wins', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const b = await new Client().open(onReplica('r2'));
    await b.hello();
    // The block stays open until every command is applied, then ends.
    await control('POST', '/blocks/hold');
    const ids = [a.set('/channels/ch4/fader', -10), a.set('/channels/ch5/fader', -11), b.set('/channels/ch4/fader', -12)];
    const applied = () => cluster.applies.filter((e) => ids.includes(e.commandId));
    while (applied().length < ids.length) await new Promise((r) => setTimeout(r, 1));
    await control('POST', '/blocks/flush');
    const delta = await a.answer<DeltaMessage>(ids[0]!);
    const origins = delta.origins!;
    expect(origins.map((o) => o.commandId).sort()).toEqual([...ids].sort());
    expect(new Set(origins.map((o) => o.owner.session))).toEqual(new Set([a.session, b.session]));
    expect(new Set(applied().map((e) => e.revision))).toEqual(new Set([delta.revision]));
    const fader4 = delta.changes!.filter((c) => c.path === '/channels/ch4/fader');
    expect(fader4).toHaveLength(1);
    const lastWriter = applied().filter((e) => e.path === '/channels/ch4/fader').at(-1)!;
    expect(fader4[0]!.value).toBe(lastWriter.value);
    expect(cluster.state.channels['ch4']!.fader).toBe(fader4[0]!.value);
  });

  it('answers a resync on a lagging replica only once it has applied lastRevision (read index)', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const sb = await new Client(a.session).open(onReplica('r2'));
    await sb.hello('standby');
    expect((await control('POST', '/replicas/r2/lag?ms=300')).status).toBe(200);
    const before = cluster.state.channels['ch7']!.fader;
    const id = a.set('/channels/ch7/fader', before === -33 ? -34 : -33);
    const delta = await a.answer<DeltaMessage>(id);
    const written = cluster.state.channels['ch7']!.fader;
    // Without lastRevision the lagging replica answers at once, older.
    const fresh = await new Client().open(onReplica('r2'));
    const old = (await fresh.hello()) as SnapshotMessage;
    expect(old.revision).toBeLessThan(delta.revision);
    expect(old.state.channels['ch7']!.fader).toBe(before);
    const from = sb.messages.length;
    sb.send({ type: 'promote', lastRevision: delta.revision });
    sb.send({ type: 'resync', lastRevision: delta.revision, full: true });
    const answer = await sb.next<SnapshotMessage>((m) => m.type === 'snapshot', from);
    expect(answer.revision).toBeGreaterThanOrEqual(delta.revision);
    expect(answer.state.channels['ch7']!.fader).toBe(written);
  });

  it('holds commands after a promote until the replica has applied its lastRevision', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const sb = await new Client(a.session).open(onReplica('r2'));
    await sb.hello('standby');
    expect((await control('POST', '/replicas/r2/lag?ms=300')).status).toBe(200);
    const delta = await a.answer<DeltaMessage>(a.set('/channels/ch8/fader', -40 - Math.random()));
    sb.send({ type: 'promote', lastRevision: delta.revision });
    const id = sb.set('/channels/ch8/mute', !cluster.state.channels['ch8']!.mute);
    await new Promise((r) => setTimeout(r, 50));
    expect(cluster.applies.some((e) => e.commandId === id)).toBe(false);
    const answer = await sb.answer<DeltaMessage>(id);
    expect(answer.revision).toBeGreaterThan(delta.revision);
  });

  it('applies a resent command id once and acks every resend as duplicate', async () => {
    await control('POST', '/audit/reset');
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const id = a.set('/channels/ch6/fader', -21, undefined, 7000);
    const delta = await a.answer<DeltaMessage>(id);
    const from = a.messages.length;
    a.set('/channels/ch6/fader', -21, id, 7000);
    const again = await a.answer<AckMessage>(id, from);
    expect(again).toMatchObject({ type: 'ack', result: 'duplicate', revision: delta.revision });
    await a.close();
    // Failover: the same session resends on another replica.
    const a2 = await new Client(a.session).open(onReplica('r2'));
    await a2.hello();
    a2.set('/channels/ch6/fader', -21, id, 7000);
    expect(await a2.answer(id)).toMatchObject({ type: 'ack', result: 'duplicate', revision: delta.revision });
    const audit = (await control<Audit>('GET', '/audit')).body;
    expect(audit.entries.filter((e) => e.commandId === id)).toEqual([
      { owner: a.you, commandId: id, type: 'set', seq: 7000, path: '/channels/ch6/fader', value: -21, revision: delta.revision, replica: 'r1' },
    ]);
    expect(audit.summary).toEqual({ total: audit.entries.length, duplicates: 0 });
    const stale = a2.set('/channels/ch6/fader', -30, undefined, 6999);
    expect(await a2.answer(stale)).toMatchObject({ type: 'ack', result: 'stale' });
  });

  it('refuses writes to a touched control from another session with 409 and the owner record', async () => {
    const a = await new Client().open();
    await a.hello();
    const b = await new Client().open();
    await b.hello();
    const path = '/channels/ch7/fader';
    const touched = await a.answer<DeltaMessage>(a.command('touch', path));
    expect(touched.owners).toEqual([{ path, owner: a.you }]);
    expect(touched.origins).toEqual([{ owner: a.you, commandId: expect.any(String) }]);
    const refused = await b.answer<ErrorMessage>(b.set(path, 0));
    expect(refused).toMatchObject({ type: 'error', code: 409, path, owner: a.you, current: cluster.state.channels['ch7']!.fader });
    expect((await b.answer<ErrorMessage>(b.command('touch', path))).code).toBe(409);
    // Another connection of the owning session writes freely.
    const a2 = await new Client(a.session).open();
    await a2.hello();
    expect((await a2.answer(a2.set(path, -1))).type).toBe('delta');
    const released = await a.answer<DeltaMessage>(a.command('release', path));
    expect(released.owners).toEqual([{ path, owner: null }]);
    expect((await b.answer(b.set(path, -2))).type).toBe('delta');
  });

  it('answers an invalid message with error 400', async () => {
    const c = await new Client().open();
    await c.hello();
    c.ws.send(JSON.stringify({ type: 'set', id: 'nope', path: '/x' }));
    expect(await c.next((m) => m.type === 'error' && m.code === 400)).toBeTruthy();
  });
});

describe('leases', () => {
  it('keeps a touch through a closed socket for less than the lease, then releases it', async () => {
    const observer = await new Client().open();
    await observer.hello();
    const a = await new Client().open();
    await a.hello();
    const path = '/channels/ch8/fader';
    await a.answer(a.command('touch', path));
    await a.close();
    const closedAt = Date.now();
    await sleep(LEASE_MS / 2);
    expect((await owners())[path]).toEqual(a.you);
    const lapse = await observer.next<DeltaMessage>(
      (m) => m.type === 'delta' && (m.owners ?? []).some((o) => o.path === path && o.owner === null),
      0,
      LEASE_MS + 1000,
    );
    const elapsed = Date.now() - closedAt;
    expect(lapse.origins).toBeUndefined();
    expect(elapsed).toBeGreaterThan(LEASE_MS - 2 * HEARTBEAT_MS);
    expect(elapsed).toBeLessThan(LEASE_MS + 3 * HEARTBEAT_MS);
    expect((await owners())[path]).toBeUndefined();
  }, 10_000);

  it('renews the lease through any message of the session on another replica', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const path = '/channels/ch9/fader';
    await a.answer(a.command('touch', path));
    await a.close();
    const a2 = await new Client(a.session).open(onReplica('r2'));
    await a2.hello('standby');
    await sleep(LEASE_MS + 600);
    expect((await owners())[path]).toEqual(a.you);
    await a2.answer(a2.command('release', path));
  }, 10_000);

  it('does not renew the lease through a partitioned replica', async () => {
    const a = await new Client().open(onReplica('r1'));
    await a.hello();
    const path = '/channels/ch10/fader';
    await a.answer(a.command('touch', path));
    await control('POST', '/replicas/r3/partition');
    const a2 = await new Client(a.session).open(onReplica('r3'));
    await a.close();
    await a2.hello('standby');
    await sleep(LEASE_MS + 600);
    expect((await owners())[path]).toBeUndefined();
  }, 10_000);
});

describe('session query', () => {
  const refused = (url: string) =>
    new Promise<number>((resolve) => {
      const ws = new WebSocket(url);
      ws.on('unexpected-response', (_req, res) => resolve(res.statusCode ?? 0));
      ws.on('open', () => {
        ws.close();
        resolve(101);
      });
    });

  it('refuses an upgrade without a valid ?session=', async () => {
    expect(await refused(`ws://${main.base}/api/v1/ws`)).toBe(400);
    expect(await refused(`ws://${main.base}/api/v1/ws?session=short`)).toBe(400);
    expect(await refused(`ws://${main.base}/api/v1/ws?session=dGFiLTEtc2Vzc2lvbi1pZA==`)).toBe(400);
    expect(await refused(`ws://${main.base}/api/v1/ws?session=dGFiLTEtc2Vzc2lvbi1pZA`)).toBe(101);
  });

  it('refuses a hello whose session differs from ?session=', async () => {
    const c = await new Client().open();
    c.send({ type: 'hello', session: 'b3RoZXItc2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary' });
    const err = await c.next<ErrorMessage>((m) => m.type === 'error');
    expect(err).toMatchObject({ code: 400, message: 'hello session differs from ?session=' });
  });
});

describe('IFB interrupt', () => {
  it('clears an active interrupt when its owner releases it', async () => {
    const c = await new Client().open();
    await c.hello();
    await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/mixMinus/mm1/interrupt/source', value: 'ch1' }] });
    await c.answer(c.command('touch', '/mixMinus/mm1/interrupt/active'));
    await c.answer(c.set('/mixMinus/mm1/interrupt/active', true));
    expect(cluster.state.mixMinus['mm1']!.interrupt.active).toBe(true);
    await c.answer(c.command('release', '/mixMinus/mm1/interrupt/active'));
    expect(cluster.state.mixMinus['mm1']!.interrupt.active).toBe(false);
  });

  it('clears an active interrupt when its owner session lease runs out', async () => {
    const c = await new Client().open();
    await c.hello();
    await c.answer(c.command('touch', '/mixMinus/mm2/interrupt/active'));
    await c.answer(c.set('/mixMinus/mm2/interrupt/active', true));
    await c.close();
    cluster.expireLeases(Date.now() + LEASE_MS + 1);
    expect(cluster.state.mixMinus['mm2']!.interrupt.active).toBe(false);
    expect(cluster.owners['/mixMinus/mm2/interrupt/active']).toBeUndefined();
  });
});

describe('heartbeats', () => {
  it('pings every connection at the heartbeat period and answers a ping with pong', async () => {
    const c = await new Client().open();
    await c.hello('standby');
    const start = c.messages.length;
    await sleep(4 * HEARTBEAT_MS + 100);
    const pings = c.messages.slice(start).filter((m) => m.type === 'ping');
    expect(pings.length).toBeGreaterThanOrEqual(3);
    expect(pings.length).toBeLessThanOrEqual(5);
    const t = pings.map((p) => (p as { t: number }).t);
    const mean = (t.at(-1)! - t[0]!) / (t.length - 1);
    expect(mean).toBeGreaterThan(HEARTBEAT_MS - 50);
    expect(mean).toBeLessThan(HEARTBEAT_MS + 50);
    c.send({ type: 'ping', t: 123456 });
    expect(await c.next((m) => m.type === 'pong' && m.t === 123456)).toMatchObject({ replica: { id: c.replica } });
  });

  it('closes a connection that sends nothing for DEAD_MS', async () => {
    const c = new Client();
    c.heartbeat = false;
    c.autoPong = false;
    await c.open();
    await c.hello();
    const helloAt = Date.now();
    await c.closed;
    const elapsed = Date.now() - helloAt;
    expect(elapsed).toBeGreaterThan(DEAD_MS - 50);
    expect(elapsed).toBeLessThan(DEAD_MS + 2 * HEARTBEAT_MS);
  });
});

describe('replica faults', () => {
  it('a partitioned replica reports itself not commitable and ignores commands until healed', async () => {
    const c = await new Client().open(onReplica('r3'));
    await c.hello();
    let from = c.messages.length;
    expect((await control('POST', '/replicas/r3/partition')).status).toBe(200);
    const status = await c.next<ReplicasMessage>((m) => m.type === 'replicas', from);
    expect(status.replicas.find((r) => r.id === 'r3')).toMatchObject({ healthy: true, commitable: false });
    const before = cluster.state.channels['ch11']!.trim;
    const id = c.set('/channels/ch11/trim', before + 1);
    await sleep(300);
    expect(c.messages.some((m) => answers(m, id))).toBe(false);
    expect(cluster.state.channels['ch11']!.trim).toBe(before);
    expect(c.count('ping', from)).toBeGreaterThanOrEqual(1);
    from = c.messages.length;
    await control('POST', '/replicas/r3/heal');
    const healed = await c.next<ReplicasMessage>((m) => m.type === 'replicas', from);
    expect(healed.replicas.find((r) => r.id === 'r3')!.commitable).toBe(true);
    c.set('/channels/ch11/trim', before + 1, id);
    expect((await c.answer(id, from)).type).toBe('delta');
  });

  it('a frozen replica sends nothing and applies nothing until unfrozen', async () => {
    await control('POST', '/audit/reset');
    const c = await new Client().open(onReplica('r2'));
    await c.hello();
    c.credit(METER_CREDIT_CAP);
    await c.next((m) => m.type === 'ping');
    expect((await control('POST', '/replicas/r2/freeze')).status).toBe(200);
    const messages = c.messages.length;
    const frames = c.frames.length;
    const id = c.set('/channels/ch12/trim', 3);
    await sleep(3 * HEARTBEAT_MS);
    expect(c.messages.length).toBe(messages);
    expect(c.frames.length).toBe(frames);
    expect(cluster.state.channels['ch12']!.trim).not.toBe(3);
    expect((await control<Audit>('GET', '/audit')).body.entries).toEqual([]);
    expect(c.ws.readyState).toBe(WebSocket.OPEN);
    await control('POST', '/replicas/r2/unfreeze');
    await c.next((m) => m.type === 'ping', messages, 2 * HEARTBEAT_MS);
    await sleep(2 * METER_PERIOD_MS);
    expect(c.frames.length).toBeGreaterThan(frames);
    expect(c.ws.readyState).toBe(WebSocket.OPEN);
  });

  it('applies commands sent to a replica that hangs after replicating them', async () => {
    const watcher = await new Client().open(onReplica('r1'));
    await watcher.hello();
    const c = await new Client().open(onReplica('r2'));
    await c.hello();
    await control('POST', '/replicas/r2/freeze?mode=apply');
    const id = c.set('/channels/ch13/trim', 4);
    await watcher.answer(id);
    await sleep(100);
    expect(c.messages.some((m) => answers(m, id))).toBe(false);
  });

  it('a killed replica closes its sockets and refuses new ones until restarted', async () => {
    const c = await new Client().open(onReplica('r1'));
    await c.hello();
    expect((await control('POST', '/replicas/r1/kill')).status).toBe(200);
    await c.closed;
    await expect(new Client().open(onReplica('r1'))).rejects.toThrow(/503/);
    const elsewhere = await new Client().open();
    await elsewhere.hello();
    expect(elsewhere.replica).not.toBe('r1');
    const replicas = (await rest('GET', '/replicas')).body!['replicas'] as Array<{ id: string; healthy: boolean; role: string }>;
    expect(replicas.find((r) => r.id === 'r1')!.healthy).toBe(false);
    expect(replicas.filter((r) => r.role === 'leader').map((r) => r.id)).not.toContain('r1');
    expect((await control('POST', '/replicas/r1/restart')).status).toBe(200);
    const back = await new Client().open(onReplica('r1'));
    await back.hello();
    expect(back.replica).toBe('r1');
  });

  it('resumes from the last seen revision, or sends a snapshot when the log has a gap', async () => {
    const a = await new Client().open();
    const snap = (await a.hello()) as SnapshotMessage;
    await a.close();
    const writer = await new Client().open();
    await writer.hello();
    await writer.answer(writer.set('/channels/ch14/trim', 1));
    await writer.answer(writer.set('/channels/ch14/trim', 2));
    const b = await new Client(a.session).open();
    const resume = await b.hello('primary', snap.revision);
    expect(resume.type).toBe('resume');
    const revisions = (resume as { deltas: DeltaMessage[] }).deltas.map((d) => d.revision);
    expect(revisions[0]).toBe(snap.revision + 1);
    expect(revisions).toEqual([...revisions].sort((x, y) => x - y));
    await control('POST', '/skip-revision');
    const c = await new Client(a.session).open();
    expect((await c.hello('primary', snap.revision)).type).toBe('snapshot');
  });
});

describe('meter frames', () => {
  it('are sent only against credit, at most the cap, and decode against the snapshot shape', async () => {
    const c = await new Client().open();
    const snap = (await c.hello()) as SnapshotMessage;
    await sleep(4 * METER_PERIOD_MS);
    expect(c.frames).toHaveLength(0);
    c.credit(3);
    await sleep(10 * METER_PERIOD_MS);
    expect(c.frames).toHaveLength(3);
    const seqs = c.frames.map((f) => f.sequence);
    expect(seqs).toEqual([seqs[0]!, seqs[0]! + 1, seqs[0]! + 2]);
    c.credit(20);
    await sleep(15 * METER_PERIOD_MS);
    expect(c.frames).toHaveLength(3 + METER_CREDIT_CAP);
    for (const f of c.frames) {
      expect(f.replica).toBe(snap.replica.id);
      expect(f.channels).toHaveLength(snap.shape.channels.length);
      expect(f.buses).toHaveLength(snap.shape.buses.length);
      expect(f.auxes).toHaveLength(snap.shape.auxes.length);
      expect(f.mixMinus).toHaveLength(snap.shape.mixMinus.length);
    }
    expect(c.frames.map((f) => f.sequence)).toEqual([...c.frames.map((f) => f.sequence)].sort((x, y) => x - y));
    expect((await statsOf(c)).metersSkippedNoCredit).toBeGreaterThan(0);
  });
});

describe('/__mock', () => {
  it('reports traffic per connection in /stats and resets it', async () => {
    await control('POST', '/stats/reset');
    const c = await new Client().open();
    await c.hello();
    c.credit(4);
    await sleep(10 * METER_PERIOD_MS);
    const first = await statsOf(c);
    expect(first).toMatchObject({
      session: c.session,
      kind: 'ui',
      mode: 'primary',
      replica: c.replica,
      meterFramesSent: 4,
      creditsOutstanding: 0,
    });
    expect(first.metersSkippedNoCredit).toBeGreaterThan(0);
    expect(first.messagesIn).toBeGreaterThanOrEqual(2);
    expect(first.messagesOut).toBeGreaterThanOrEqual(c.messages.length + 4 - 1);
    expect(Date.parse(first.openedAt)).toBeLessThanOrEqual(Date.now());
    await sleep(2 * HEARTBEAT_MS);
    const stats = (await control<Stats>('GET', '/stats')).body;
    const second = stats.connections.find((r) => r.connection === c.you!.connection)!;
    for (const k of ['meterFramesSent', 'metersSkippedNoCredit', 'messagesIn', 'messagesOut'] as const) {
      expect(second[k]).toBeGreaterThanOrEqual(first[k]);
    }
    expect(second.messagesIn).toBeGreaterThan(first.messagesIn);
    expect(stats.totals.meterFramesSent).toBeGreaterThanOrEqual(4);
    expect(stats.totals.connections).toBe(stats.connections.length);
    await control('POST', '/stats/reset');
    const reset = await statsOf(c);
    expect(reset.meterFramesSent).toBe(0);
    expect(reset.messagesIn).toBeLessThan(second.messagesIn);
  });

  it('lists replica state and refuses unknown replicas', async () => {
    await control('POST', '/replicas/r2/partition');
    const rows = (await control<Array<Record<string, unknown>>>('GET', '/replicas')).body;
    expect(rows.find((r) => r['id'] === 'r2')).toMatchObject({ healthy: true, frozen: 'none', partitioned: true });
    expect((await control('POST', '/replicas/r9/kill')).status).toBe(404);
  });
});

describe('shape', () => {
  it('generates the requested channel count', () => {
    expect(exampleShape().channels).toHaveLength(DEFAULT_CHANNELS);
    for (const n of [1, 8, 64]) {
      const ids = exampleShape(n).channels.map((c) => c.id);
      expect(ids).toHaveLength(n);
      expect(new Set(ids).size).toBe(n);
    }
  });

  it('serves 64 channels when the cluster is built with them', async () => {
    const wide = await startMock({ automation: false, replicas: 2, channels: 64 });
    try {
      const c = await new Client().open('/api/v1/ws', wide.base);
      const snap = (await c.hello()) as SnapshotMessage;
      expect(snap.shape.channels).toHaveLength(64);
      expect(contract.ajv.validate({ $ref: 'https://github.com/danielclasen/faderz/raw/main/api/ws/model.schema.json#/$defs/State' }, snap.state)).toBe(true);
      c.credit(2);
      await sleep(6 * METER_PERIOD_MS);
      expect(c.frames).toHaveLength(2);
      expect(c.frames[0]!.channels).toHaveLength(64);
      await c.close();
    } finally {
      await wide.stop();
    }
  });
});

describe('REST', () => {
  it('reads the state document', async () => {
    const res = await rest('GET', '/state');
    expect(res.status).toBe(200);
    expect(res.body).toMatchObject({ revision: cluster.pendingRevision });
  });

  it('patches atomically, once per command id', async () => {
    const commandId = crypto.randomUUID();
    const body = { commandId, changes: [{ path: '/channels/ch15/fader', value: -4 }, { path: '/channels/ch15/mute', value: true }] };
    const first = await rest('PATCH', '/state', body);
    expect(first).toMatchObject({ status: 200, body: { result: 'applied', commandId } });
    const second = await rest('PATCH', '/state', body);
    expect(second).toMatchObject({ status: 200, body: { result: 'duplicate', revision: first.body!['revision'] } });
  });

  it('refuses owned controls with 409, the current value and the owner record', async () => {
    const c = await new Client().open();
    await c.hello();
    await c.answer(c.command('touch', '/channels/ch16/fader'));
    const res = await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch16/fader', value: 0 }] });
    expect(res).toMatchObject({ status: 409, body: { conflicts: [{ path: '/channels/ch16/fader', owner: c.you, current: -90 }] } });
    await c.answer(c.command('release', '/channels/ch16/fader'));
  });

  it('refuses invalid bodies, unknown controls and requests without the CSRF header', async () => {
    expect((await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch1/fader', value: 20 }] })).status).toBe(400);
    expect((await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch99/fader', value: 0 }] })).status).toBe(404);
    const res = await fetch(`http://${main.base}/api/v1/state`, { method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: '{}' });
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

  it('inverts one leg of a stereo channel and leaves the other, and refuses a leg beyond the width', async () => {
    const ok = await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/st1/polarity/1', value: true }] });
    expect(ok.status).toBe(200);
    expect(cluster.state.channels['st1']!.polarity).toEqual([false, true]);
    const beyond = await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch1/polarity/1', value: true }] });
    expect(beyond.status).toBe(404);
    await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/st1/polarity/1', value: false }] });
  });

  it('recalls a scene as one change per leg', async () => {
    const created = await rest('POST', '/scenes', { commandId: crypto.randomUUID(), name: 'Legs' });
    const id = (created.body!['scene'] as { id: string }).id;
    await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/sur1/legTrim/2', value: 4 }] });
    const c = await new Client().open();
    await c.hello();
    await rest('POST', `/scenes/${id}/recall`, { commandId: crypto.randomUUID(), crossfadeMs: 0 });
    const delta = await c.next<DeltaMessage>((m) => m.type === 'delta' && (m.changes ?? []).some((x) => x.path.includes('legTrim')));
    expect(delta.changes).toContainEqual({ path: '/channels/sur1/legTrim/2', value: 0 });
    expect(cluster.state.channels['sur1']!.legTrim).toEqual([0, 0, 0, 0, 0, 0]);
    await rest('DELETE', `/scenes/${id}`);
  });

  it('recalls the direct out tap with routing and never the meter tap', async () => {
    const created = await rest('POST', '/scenes', { commandId: crypto.randomUUID(), name: 'Taps' });
    const id = (created.body!['scene'] as { id: string }).id;
    await rest('PATCH', '/state', {
      commandId: crypto.randomUUID(),
      changes: [{ path: '/channels/ch1/directOut/tap', value: 'input' }, { path: '/channels/ch1/meterTap', value: 'postFader' }],
    });
    await rest('POST', `/scenes/${id}/recall`, { commandId: crypto.randomUUID(), crossfadeMs: 0 });
    expect(cluster.state.channels['ch1']!.directOut.tap).toBe('postFader');
    expect(cluster.state.channels['ch1']!.meterTap).toBe('postFader');
    await rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path: '/channels/ch1/meterTap', value: 'preFader' }] });
    await rest('DELETE', `/scenes/${id}`);
  });

  it('applies a loudness preset with its target and tolerance, and a typed target makes it custom', async () => {
    const write = (path: string, value: unknown) =>
      rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path, value }] });
    expect((await write('/metering/loudness/preset', 'atsc')).status).toBe(200);
    expect(cluster.state.metering.loudness).toEqual({ preset: 'atsc', target: -24, tolerance: 2 });
    await write('/metering/loudness/target', -27);
    expect(cluster.state.metering.loudness).toEqual({ preset: 'custom', target: -27, tolerance: 2 });
    await write('/metering/loudness/preset', 'ebu');
    expect(cluster.state.metering.loudness).toEqual({ preset: 'ebu', target: -23, tolerance: 1 });
  });

  it('stores and recalls AFV timing and closed level with a scene', async () => {
    const write = (path: string, value: unknown) => rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path, value }] });
    await write('/afv/ch13/fallMs', 800);
    const created = await rest('POST', '/scenes', { commandId: crypto.randomUUID(), name: 'AFV' });
    const id = (created.body!['scene'] as { id: string }).id;
    await write('/afv/ch13/fallMs', 100);
    await write('/afv/ch13/offLevel', -15);
    await rest('POST', `/scenes/${id}/recall`, { commandId: crypto.randomUUID(), crossfadeMs: 0 });
    expect(cluster.state.afv['ch13']).toMatchObject({ fallMs: 800, offLevel: -90 });
    await write('/afv/ch13/fallMs', 200);
    await rest('DELETE', `/scenes/${id}`);
  });

  it('switches a channel to its B input and refuses B where none is configured', async () => {
    const write = (path: string, value: unknown) => rest('PATCH', '/state', { commandId: crypto.randomUUID(), changes: [{ path, value }] });
    expect((await write('/channels/ch1/input', 'b')).status).toBe(200);
    expect(cluster.state.channels['ch1']!.input).toBe('b');
    expect((await write('/channels/ch5/input', 'b')).status).toBe(400);
    await write('/channels/ch1/input', 'a');
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
