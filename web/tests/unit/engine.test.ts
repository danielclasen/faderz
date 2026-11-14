import { describe, expect, it } from 'vitest';
import { SyncEngine, STANDBY_RETRY_MS } from '../../src/lib/sync/engine';
import type { Patch } from '../../src/lib/sync/messages';
import { deriveMeterLayout, encodeMeterFrame, type MeterLayout, type MeterValues } from '../../src/lib/contract/meterFrame';
import { DEAD_MS, HEARTBEAT_MS, METER_CREDIT_CAP, SUSPECT_MS } from '../../src/lib/contract/protocol';
import type {
  DeltaMessage,
  Owner,
  Owners,
  ReplicaRef,
  ReplicasMessage,
  ResumeMessage,
  SetMessage,
  SnapshotMessage,
} from '../../src/lib/contract/types';
import { exampleShape, initialState } from '../../dev/mock/shape';
import { FakeClock, FakeSocket } from './fakes';

const R1: ReplicaRef = { id: 'r1', role: 'leader' };
const R2: ReplicaRef = { id: 'r2', role: 'follower' };
const R3: ReplicaRef = { id: 'r3', role: 'follower' };
const SESSION = 'sess-me';
const YOU: Owner = { principal: 'anonymous', session: SESSION, kind: 'ui', connection: 'c1' };
const OTHER: Owner = { principal: 'anonymous', session: 'sess-other', kind: 'ui', connection: 'c9' };
const F1 = '/channels/ch1/fader';
const F2 = '/channels/ch2/fader';
const F3 = '/channels/ch3/fader';
const SEQ_BASE = 1_700_000_000_000;

function setup(endpoints = ['ws://svc/api/v1/ws'], seqBase = SEQ_BASE) {
  const clock = new FakeClock();
  const sockets: FakeSocket[] = [];
  const patches: Patch[] = [];
  const engine = new SyncEngine({
    endpoints,
    session: SESSION,
    seqBase,
    socket: (url) => {
      const s = new FakeSocket(url);
      sockets.push(s);
      return s;
    },
    clock,
    post: (p) => patches.push(structuredClone(p)),
  });
  engine.start();
  const tick = () => {
    const before = patches.length;
    engine.input({ type: 'tick' });
    return patches.length > before ? patches.at(-1)! : null;
  };
  return { clock, sockets, patches, engine, tick };
}

function replicas(from: ReplicaRef, notCommitable: string[] = []): ReplicasMessage {
  return {
    type: 'replicas',
    replica: from,
    replicas: [R1, R2, R3].map((r) => ({ ...r, healthy: true, commitable: !notCommitable.includes(r.id) })),
  };
}

function snapshot(replica: ReplicaRef, revision: number, owners: Owners = {}, you: Owner = YOU): SnapshotMessage {
  const shape = exampleShape();
  return { type: 'snapshot', you, replica, revision, shape, state: initialState(shape), owners };
}

function resume(replica: ReplicaRef, revision: number, deltas: DeltaMessage[] = []): ResumeMessage {
  return { type: 'resume', you: { ...YOU, connection: 'c2' }, replica, revision, deltas };
}

function delta(replica: ReplicaRef, revision: number, extra: Partial<DeltaMessage>): DeltaMessage {
  return { type: 'delta', replica, revision, ...extra };
}

/** Primary on r1 synced from a snapshot, standby on r2 ready. */
function live(endpoints?: string[], owners: Owners = {}) {
  const t = setup(endpoints);
  const primary = t.sockets[0]!;
  primary.open();
  primary.deliver(replicas(R1));
  primary.deliver(snapshot(R1, 10, owners));
  const standby = t.sockets[1]!;
  standby.open();
  standby.deliver(replicas(R2));
  standby.deliver(resume(R2, 10));
  expect(t.engine.link).toBe('Live');
  expect(t.engine.standbyState).toBe('Ready');
  return { ...t, primary, standby };
}

/** Advance time in heartbeat steps; the given sockets send a ping each step. */
function beats(clock: FakeClock, ms: number, ...peers: Array<[FakeSocket, ReplicaRef]>) {
  for (let left = ms; left > 0; left -= HEARTBEAT_MS) {
    clock.advance(Math.min(HEARTBEAT_MS, left));
    for (const [s, r] of peers) s.deliver({ type: 'ping', replica: r, t: clock.now() });
  }
}

function meterValues(layout: MeterLayout, db: number): MeterValues {
  const legs = (n: number) => Array.from({ length: n }, () => db);
  const bus = (n: number) => ({
    peak: legs(n),
    truePeak: legs(n),
    rms: legs(n),
    loudness: { momentary: -23, shortTerm: null, integrated: null, lra: null, integratedValid: false },
  });
  return {
    channels: layout.channels.map((e) => ({ peak: legs(e.legs), rms: legs(e.legs) })),
    buses: layout.buses.map((e) => bus(e.legs)),
    auxes: layout.auxes.map((e) => bus(e.legs)),
    mixMinus: layout.mixMinus.map((e) => bus(e.legs)),
  };
}

describe('sync engine: connecting', () => {
  it('says hello as a primary ui session, goes live on the snapshot and opens a standby', () => {
    const { sockets, engine, tick } = setup();
    expect(sockets).toHaveLength(1);
    sockets[0]!.open();
    expect(sockets[0]!.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'primary' });
    expect(engine.link).toBe('Syncing');
    sockets[0]!.deliver(replicas(R1));
    sockets[0]!.deliver(snapshot(R1, 4));
    expect(engine.link).toBe('Live');
    expect(sockets).toHaveLength(2);
    sockets[1]!.open();
    expect(sockets[1]!.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'standby', lastRevision: 4 });
    const p = tick()!;
    expect(p.snapshot?.state.channels['ch1']).toBeDefined();
    expect(p.status).toMatchObject({ link: 'Live', primaryReplica: 'r1', standby: 'Syncing', revision: 4, session: SESSION });
  });

  it('opens the standby on another endpoint of a list', () => {
    const { primary, standby } = live(['ws://a/ws', 'ws://b/ws', 'ws://c/ws']);
    expect(primary.url).toBe('ws://a/ws');
    expect(standby.url).not.toBe('ws://a/ws');
  });

  it('retries the standby when it lands on the primary replica', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    const first = t.sockets[1]!;
    first.open();
    first.deliver(replicas(R1));
    expect(first.closed).toBe(true);
    expect(t.engine.standbyState).toBe('Idle');
    t.clock.advance(STANDBY_RETRY_MS);
    const second = t.sockets[2]!;
    second.open();
    second.deliver(replicas(R1));
    expect(second.closed).toBe(true);
    t.clock.advance(STANDBY_RETRY_MS);
    const third = t.sockets[3]!;
    third.open();
    third.deliver(replicas(R3));
    third.deliver(resume(R3, 1));
    expect(t.engine.standbyState).toBe('Ready');
    expect(t.engine.status().standbyReplica).toBe('r3');
  });

  it('sends no meter frames request and no commands on the standby', () => {
    const { standby, engine, tick } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    tick();
    expect(standby.sent.map((m) => m.type)).toEqual(['hello']);
  });
});

describe('sync engine: heartbeat and failure detection', () => {
  it('pings both connections every 250 ms and answers pings with pong', () => {
    const { clock, primary, standby } = live();
    beats(clock, 1000, [primary, R1], [standby, R2]);
    expect(primary.ofType('ping')).toHaveLength(4);
    expect(standby.ofType('ping')).toHaveLength(4);
    primary.deliver({ type: 'ping', replica: R1, t: 4242 });
    expect(primary.sent.at(-1)).toEqual({ type: 'pong', t: 4242 });
  });

  it('marks a primary late by one beat suspect and fails nothing over', () => {
    const { clock, engine, primary, standby } = live();
    clock.advance(SUSPECT_MS + 10);
    standby.deliver({ type: 'ping', replica: R2, t: 1 });
    expect(engine.link).toBe('Suspect');
    primary.deliver({ type: 'ping', replica: R1, t: 2 });
    expect(engine.link).toBe('Live');
    beats(clock, 2000, [primary, R1], [standby, R2]);
    expect(engine.link).toBe('Live');
    expect(standby.ofType('promote')).toHaveLength(0);
    expect(primary.closed).toBe(false);
  });

  it('fails over after three missed beats', () => {
    const { clock, engine, primary, standby, tick } = live();
    beats(clock, DEAD_MS - 50, [standby, R2]);
    expect(engine.link).toBe('Suspect');
    expect(standby.ofType('promote')).toHaveLength(0);
    beats(clock, 100, [standby, R2]);
    expect(primary.closed).toBe(true);
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(standby.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 10 }]);
    expect(engine.link).toBe('Resyncing');
    expect(tick()!.status!.failover).toEqual({ n: 1, from: 'r1', to: 'r2', reason: 'dead' });
  });

  it('fails over at once when the primary socket is lost', () => {
    const { engine, primary, standby } = live();
    primary.drop();
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(engine.primary?.socket).toBe(standby);
    standby.deliver(resume(R2, 10));
    expect(engine.link).toBe('Live');
  });

  it('fails over when the serving replica reports it cannot commit', () => {
    const { engine, primary, standby } = live();
    primary.deliver(replicas(R1, ['r1']));
    expect(primary.closed).toBe(true);
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(engine.status().failover?.reason).toBe('notCommitable');
  });

  it('stays on a non-committing primary while no standby is ready', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1, ['r1']));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    expect(t.engine.link).toBe('Live');
    expect(t.sockets[0]!.closed).toBe(false);
    const sb = t.sockets[1]!;
    sb.open();
    sb.deliver(replicas(R2, ['r1']));
    sb.deliver(resume(R2, 1));
    expect(t.sockets[0]!.closed).toBe(true);
    expect(sb.ofType('promote')).toHaveLength(1);
  });

  it('opens a new standby on another replica after a promote, retrying until it does', () => {
    const { clock, sockets, standby } = live();
    sockets[0]!.drop();
    standby.deliver(resume(R2, 10));
    const next = sockets[2]!;
    next.open();
    expect(next.sent[0]).toMatchObject({ type: 'hello', mode: 'standby' });
    next.deliver(replicas(R2));
    expect(next.closed).toBe(true);
    clock.advance(STANDBY_RETRY_MS);
    sockets[3]!.open();
    sockets[3]!.deliver(replicas(R3));
    sockets[3]!.deliver(resume(R3, 10));
    expect(sockets[3]!.closed).toBe(false);
  });

  it('reconnects through the endpoints when no standby is ready', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    t.sockets[0]!.drop();
    expect(t.engine.link).toBe('Connecting');
    t.clock.advance(1000);
    const again = t.sockets.find((s, i) => i > 0 && s.url === 'ws://b/ws' && s.onopen)!;
    again.open();
    expect(again.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'primary', lastRevision: 3 });
  });
});

describe('sync engine: commands', () => {
  it('resends every unanswered command on the promoted standby with the same ids and no others', () => {
    const { engine, primary, standby, clock } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    clock.advance(30);
    engine.input({ type: 'set', path: F2, value: -6 });
    engine.input({ type: 'touch', path: F3 });
    const sent = primary.sent.filter((m) => m.type === 'set' || m.type === 'touch') as Array<SetMessage | { type: 'touch'; id: string }>;
    expect(sent).toHaveLength(3);
    const [a, b, c] = sent;
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -5 }], origins: [{ owner: YOU, commandId: a!.id, seq: (a as SetMessage).seq }] }));
    primary.drop();
    const resent = standby.sent.filter((m) => m.type === 'set' || m.type === 'touch');
    expect(resent).toEqual([b, c]);
    expect(standby.sent.findIndex((m) => m.type === 'promote')).toBeLessThan(standby.sent.findIndex((m) => m.type === 'set'));
    standby.deliver(resume(R2, 11));
    expect(standby.sent.filter((m) => m.type === 'set' || m.type === 'touch')).toHaveLength(2);
  });

  it('resends a sent command after a snapshot that skipped its delta, and posts server values once it is answered', () => {
    const { engine, primary, tick } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = primary.ofType('set');
    primary.deliver(snapshot(R1, 15));
    const sets = primary.ofType('set');
    expect(sets).toHaveLength(2);
    expect(sets[1]!.id).toBe(set!.id);
    primary.deliver({ type: 'ack', replica: R1, commandId: set!.id, result: 'duplicate', revision: 12 });
    expect(engine.outbox.size).toBe(0);
    tick();
    primary.deliver(delta(R1, 16, { changes: [{ path: F1, value: -9 }] }));
    expect(tick()?.changes).toContainEqual([F1, -9]);
  });

  it('answers a command from a delta seen on the standby, so it is not resent', () => {
    const { engine, primary, standby } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = primary.ofType('set');
    standby.deliver(delta(R2, 11, { changes: [{ path: F1, value: -5 }], origins: [{ owner: { ...YOU, connection: 'c1' }, commandId: set!.id }] }));
    expect(engine.outbox.size).toBe(0);
    primary.drop();
    expect(standby.ofType('set')).toHaveLength(0);
  });

  it('starts every control seq at the load time and keeps it above the last load after a reload', () => {
    const a = setup(['ws://svc/ws'], SEQ_BASE);
    a.sockets[0]!.open();
    a.sockets[0]!.deliver(snapshot(R1, 1));
    for (let i = 0; i < 50; i++) {
      a.engine.input({ type: 'set', path: F1, value: -i });
      a.clock.advance(20);
    }
    const seqA = a.sockets[0]!.ofType('set').map((m) => m.seq);
    expect(seqA[0]).toBe(SEQ_BASE + 1);
    for (let i = 1; i < seqA.length; i++) expect(seqA[i]!).toBeGreaterThan(seqA[i - 1]!);
    // Reload one second later: the new page starts from its own load time.
    const b = setup(['ws://svc/ws'], SEQ_BASE + 1000);
    b.sockets[0]!.open();
    b.sockets[0]!.deliver(snapshot(R1, 60));
    b.engine.input({ type: 'set', path: F1, value: 1 });
    expect(b.sockets[0]!.ofType('set')[0]!.seq).toBeGreaterThan(Math.max(...seqA));
  });

  it('limits writes of one control to 50 per second and sends the trailing value', () => {
    const { engine, primary, clock } = live();
    for (let i = 0; i < 100; i++) {
      engine.input({ type: 'set', path: F1, value: -i / 10 });
      clock.advance(5);
    }
    clock.advance(40);
    const sets = primary.ofType('set');
    expect(sets.length).toBeLessThanOrEqual(26);
    expect(sets.at(-1)!.value).toBe(-9.9);
  });

  it('queues commands while disconnected, coalescing continuous controls and keeping discrete ones', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(snapshot(R1, 1));
    t.sockets[0]!.drop();
    for (let i = 0; i < 5; i++) {
      t.engine.input({ type: 'set', path: F1, value: -i });
      t.clock.advance(25);
    }
    t.engine.input({ type: 'set', path: '/channels/ch1/mute', value: true });
    t.clock.advance(25);
    t.engine.input({ type: 'set', path: '/channels/ch1/mute', value: false });
    t.clock.advance(2000);
    const s = t.sockets.find((x) => x !== t.sockets[0] && x.onopen)!;
    s.open();
    s.deliver(resume(R2, 1));
    expect(s.ofType('set').map((m) => [m.path, m.value])).toEqual([
      [F1, -4],
      ['/channels/ch1/mute', true],
      ['/channels/ch1/mute', false],
    ]);
  });
});

describe('sync engine: ownership', () => {
  it('releases on resync every control the session owns but is not touching', () => {
    const t = setup();
    t.engine.input({ type: 'touch', path: F2 });
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(snapshot(R1, 5, { [F1]: { ...YOU, connection: 'old' }, [F2]: { ...YOU, connection: 'old' }, [F3]: OTHER }));
    expect(t.sockets[0]!.ofType('release').map((m) => m.path)).toEqual([F1]);
  });

  it('releases on the resume after a failover too, and not twice', () => {
    const { engine, primary, standby } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver(delta(R1, 11, { owners: [{ path: F1, owner: YOU }], origins: [{ owner: YOU, commandId: touch!.id }] }));
    engine.input({ type: 'release', path: F1 });
    primary.drop();
    standby.deliver(resume(R2, 11));
    expect(standby.ofType('release').map((m) => m.path)).toEqual([F1]);
  });

  it('keeps a touch across a failover and touches again on the new primary if ownership lapsed', () => {
    const { engine, primary, standby } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver(delta(R1, 11, { owners: [{ path: F1, owner: YOU }], origins: [{ owner: YOU, commandId: touch!.id }] }));
    primary.drop();
    expect(engine.touches.get(F1)).toBe('Touching');
    standby.deliver(resume(R2, 12, [delta(R2, 11, { owners: [{ path: F1, owner: YOU }] }), delta(R2, 12, { owners: [{ path: F1, owner: null }] })]));
    expect(engine.touches.get(F1)).toBe('Touching');
    expect(standby.ofType('touch').map((m) => m.path)).toEqual([F1]);
    expect(standby.ofType('release')).toHaveLength(0);
  });

  it('forgets a refused touch so pointer-up sends no release', () => {
    const { engine, primary } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver({ type: 'error', replica: R1, code: 409, message: 'owned', commandId: touch!.id, path: F1, current: 0, owner: OTHER });
    expect(engine.touches.has(F1)).toBe(false);
    engine.input({ type: 'release', path: F1 });
    expect(primary.ofType('release')).toHaveLength(0);
  });
});

describe('sync engine: patches', () => {
  it('holds back server values of a control with a pending write, then posts the committed value', () => {
    const { engine, primary, tick } = live();
    tick();
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = primary.ofType('set');
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -20 }, { path: F2, value: -3 }] }));
    const p1 = tick()!;
    expect(p1.changes).toEqual([[F2, -3]]);
    primary.deliver(delta(R1, 12, { changes: [{ path: F1, value: -5 }], origins: [{ owner: YOU, commandId: set!.id }] }));
    expect(tick()!.changes).toEqual([[F1, -5]]);
  });

  it('restores the committed value after a refused write and reports the owner', () => {
    const { engine, primary, tick } = live();
    tick();
    engine.input({ type: 'set', path: F1, value: 4 });
    const [set] = primary.ofType('set');
    primary.deliver({ type: 'error', replica: R1, code: 409, message: 'owned', commandId: set!.id, path: F1, current: 0, owner: OTHER });
    const p = tick()!;
    expect(p.changes).toEqual([[F1, 0]]);
    expect(p.notices).toEqual([{ kind: 'conflict', path: F1, owner: OTHER }]);
  });

  it('carries pending local values in a snapshot patch', () => {
    const { engine, primary, tick } = live();
    tick();
    engine.input({ type: 'set', path: F1, value: -7 });
    primary.deliver(snapshot(R1, 30));
    const p = tick()!;
    expect(p.snapshot).toBeDefined();
    expect(p.changes).toEqual([[F1, -7]]);
    expect(p.input).toBe(3);
  });

  it('posts nothing on a tick with nothing new', () => {
    const { tick } = live();
    expect(tick()).not.toBeNull();
    expect(tick()).toBeNull();
  });

  it('sends resync with lastRevision on a revision gap', () => {
    const { engine, primary } = live();
    primary.deliver(delta(R1, 12, { changes: [{ path: F1, value: 1 }] }));
    expect(primary.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 10 }]);
    expect(engine.link).toBe('Resyncing');
  });
});

describe('sync engine: meters', () => {
  const layout = deriveMeterLayout(exampleShape());
  const frame = (seq: number, l: MeterLayout = layout) =>
    encodeMeterFrame(l, { sequence: seq, replica: 'r1' }, meterValues(l, -20));

  it('grants credits only on ticks, at most the cap outstanding, batched', () => {
    const { clock, primary, tick, engine } = live();
    beats(clock, 2000, [primary, R1]);
    expect(primary.ofType('credit')).toHaveLength(0);
    tick();
    expect(primary.ofType('credit')).toEqual([{ type: 'credit', n: METER_CREDIT_CAP }]);
    for (let i = 0; i < 10; i++) tick();
    expect(primary.ofType('credit')).toHaveLength(1);
    for (let i = 0; i < 3; i++) primary.deliver(frame(i));
    tick();
    expect(primary.ofType('credit')).toHaveLength(1);
    primary.deliver(frame(3));
    tick();
    expect(primary.ofType('credit').at(-1)).toEqual({ type: 'credit', n: 4 });
    expect(engine.credits).toBe(METER_CREDIT_CAP);
    const granted = primary.ofType('credit').reduce((s, c) => s + c.n, 0);
    expect(granted - 4).toBeLessThanOrEqual(METER_CREDIT_CAP);
  });

  it('decodes a frame and posts only the latest one per tick, with its replica', () => {
    const { primary, tick } = live();
    tick();
    primary.deliver(frame(1));
    primary.deliver(frame(2));
    const p = tick()!;
    expect(p.meters?.replica).toBe('r1');
    expect(p.meters?.sequence).toBe(2);
    expect(p.meters?.values).toBeInstanceOf(Float32Array);
    expect(p.meters?.values.length).toBe(layout.valueCount);
    expect(p.meters?.values[0]).toBeCloseTo(-20);
    expect(tick()?.meters).toBeUndefined();
  });

  it('sends resync without lastRevision once on a layout fingerprint mismatch', () => {
    const { primary } = live();
    const other = deriveMeterLayout(exampleShape(3));
    primary.deliver(frame(1, other));
    primary.deliver(frame(2, other));
    expect(primary.ofType('resync')).toEqual([{ type: 'resync' }]);
  });
});
