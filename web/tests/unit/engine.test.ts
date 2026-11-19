import { describe, expect, it } from 'vitest';
import {
  COMMIT_SETTLE_MS,
  CONNECT_TIMEOUT_CAP_MS,
  CONNECT_TIMEOUT_MS,
  FIRST_CONNECT_TIMEOUT_MS,
  NOT_APPLIED_RETRY_MS,
  OFFLINE_RETRY_MS,
  PRIMARY_RETRY_MS,
  RESYNC_RETRY_MS,
  RETRY_CAP_MS,
  SLACK_CAP_MS,
  SYNC_TIMEOUT_CAP_MS,
  SYNC_TIMEOUT_MS,
  SyncEngine,
  STANDBY_BACKOFF_MS,
  STANDBY_MISSES,
  STANDBY_RETRY_MS,
  STANDBY_TRIES,
} from '../../src/lib/sync/engine';
import { getAt } from '../../src/lib/state/pointer';
import { ownerHandle } from '../../src/lib/net/handle';
import type { Patch } from '../../src/lib/sync/messages';
import { deriveMeterLayout, encodeMeterFrame, type MeterLayout, type MeterValues } from '../../src/lib/contract/meterFrame';
import { COMMAND_EXPIRY_MS, DEAD_MS, HEARTBEAT_MS, MAX_COMMANDS_IN_FLIGHT, METER_CREDIT_CAP, SUSPECT_MS } from '../../src/lib/contract/protocol';
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
const SESSION = 'c2Vzc2lvbi1tZS0xMjM0NTY';
const YOU: Owner = { principal: 'anonymous', handle: ownerHandle(SESSION), kind: 'ui', connection: 'c1' };
const OTHER: Owner = { principal: 'anonymous', handle: ownerHandle('b3RoZXItc2Vzc2lvbi0xMjM'), kind: 'ui', connection: 'c9' };
const F1 = '/channels/ch1/fader';
const F2 = '/channels/ch2/fader';
const F3 = '/channels/ch3/fader';
const SEQ_BASE = 1_700_000_000_000;
/** Above 2^53: an epoch is compared as text, never as a number. */
const EPOCH = '18446744073709551557';
const RESTORED = '18446744073709551556';

function setup(endpoints = ['ws://svc/api/v1/ws'], seqBase = SEQ_BASE, random = () => 0) {
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
    random,
  });
  engine.start();
  const tick = () => {
    const before = patches.length;
    engine.input({ type: 'tick' });
    return patches.length > before ? patches.at(-1)! : null;
  };
  return { clock, sockets, patches, engine, tick };
}

/** Each replica a build of its own, so a test tells whose a status names. */
const buildOf = (r: ReplicaRef) => ({ faderz: `0.4.${r.id.slice(1)}`, samplez: '0.12.3' });

function replicas(from: ReplicaRef, notCommitable: string[] = []): ReplicasMessage {
  return {
    type: 'replicas',
    replica: from,
    build: buildOf(from),
    replicas: [R1, R2, R3].map((r) => ({ ...r, healthy: true, commitable: !notCommitable.includes(r.id) })),
  };
}

function snapshot(replica: ReplicaRef, revision: number, owners: Owners = {}, you: Owner = YOU, epoch = EPOCH): SnapshotMessage {
  const shape = exampleShape();
  return { type: 'snapshot', you, replica, epoch, revision, shape, state: initialState(shape), owners };
}

function resume(replica: ReplicaRef, revision: number, deltas: DeltaMessage[] = [], epoch = EPOCH): ResumeMessage {
  return { type: 'resume', you: { ...YOU, connection: 'c2' }, replica, epoch, revision, deltas };
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
    loudness: { momentary: -23, shortTerm: null, integrated: null, lra: null, maxMomentary: -23, maxShortTerm: null, integratedValid: false, lraStable: false },
    correlation: null,
  });
  return {
    channels: layout.channels.map((e) => ({ peak: legs(e.legs), truePeak: legs(e.legs), momentary: db })),
    buses: layout.buses.map((e) => bus(e.legs)),
    auxes: layout.auxes.map((e) => bus(e.legs)),
    mixMinus: layout.mixMinus.map((e) => bus(e.legs)),
    monitor: layout.monitor.map((e) => ({ peak: legs(e.legs) })),
    ducks: layout.ducks.map(() => ({ key: db, envelope: 0.5, engaged: true })),
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
    expect(sockets[1]!.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'standby', lastRevision: 4, epoch: EPOCH });
    const p = tick()!;
    expect(p.snapshot?.state.channels['ch1']).toBeDefined();
    expect(p.status).toMatchObject({ link: 'Live', primaryReplica: 'r1', standby: 'Syncing', revision: 4, session: SESSION, handle: ownerHandle(SESSION) });
  });

  it('opens the standby on another endpoint of a list', () => {
    const { primary, standby } = live(['ws://a/ws', 'ws://b/ws', 'ws://c/ws']);
    expect(primary.url).toBe(`ws://a/ws?session=${SESSION}`);
    expect(standby.url).not.toBe(`ws://a/ws?session=${SESSION}`);
  });

  it('carries the same ?session= on the primary, the standby, every reconnect and after a failover', () => {
    const t = live(['ws://a/ws', 'ws://b/ws', 'ws://c/ws']);
    t.primary.drop();
    t.clock.advance(5000);
    const urls = t.sockets.map((s) => new URL(s.url));
    expect(urls.length).toBeGreaterThan(2);
    for (const u of urls) expect(u.searchParams.getAll('session')).toEqual([SESSION]);
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

  /**
   * Primary synced on r1; then one standby per step: it lands on r1 and
   * reports the step's message, or the server drops it once open. Stops
   * at the first standby not retried after STANDBY_RETRY_MS.
   */
  function standbyLandings(steps: ReadonlyArray<ReplicasMessage | 'drop'>) {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    for (const [i, step] of steps.entries()) {
      const s = t.sockets.at(-1)!;
      expect(s).not.toBe(t.sockets[0]);
      s.open();
      if (step === 'drop') s.drop();
      else s.deliver(step);
      expect(s.closed).toBe(true);
      const before = t.sockets.length;
      t.clock.advance(STANDBY_RETRY_MS);
      t.sockets[0]!.deliver({ type: 'ping', replica: R1, t: t.clock.now() });
      if (t.sockets.length === before) return { ...t, retried: i };
    }
    return { ...t, retried: steps.length };
  }

  /** `n` standbys that each land on r1 and report `report`. */
  const standbyMisses = (n: number, report: ReplicasMessage) => standbyLandings(Array<ReplicasMessage>(n).fill(report));

  // A service endpoint routes every connection at random: landing on the
  // primary's replica again and again while another is healthy is chance,
  // and backing off for it left pages without a standby for 30 s.
  it('retries a standby at once while it lands on the primary replica and another replica is healthy', () => {
    const t = standbyMisses(STANDBY_MISSES, replicas(R1));
    expect(t.retried).toBe(STANDBY_MISSES);
    const s = t.sockets.at(-1)!;
    s.open();
    s.deliver(replicas(R2));
    s.deliver(resume(R2, 1));
    expect(t.engine.standbyState).toBe('Ready');
    expect(t.clock.now()).toBe(STANDBY_MISSES * STANDBY_RETRY_MS);
  });

  it('backs off a standby that keeps landing on the primary replica past the misses, or when no other is healthy', () => {
    for (const [report, fast] of [
      [replicas(R1), STANDBY_MISSES + STANDBY_TRIES],
      [{ ...replicas(R1), replicas: replicas(R1).replicas.map((r) => ({ ...r, healthy: r.id === 'r1' })) }, STANDBY_TRIES],
    ] as const) {
      const t = standbyMisses(fast + 1, report);
      expect(t.retried).toBe(fast - 1);
      const before = t.sockets.length;
      beats(t.clock, STANDBY_BACKOFF_MS - STANDBY_RETRY_MS - 1, [t.sockets[0]!, R1]);
      expect(t.sockets).toHaveLength(before);
      t.clock.advance(1);
      expect(t.sockets).toHaveLength(before + 1);
    }
  });

  // The cap bounds misses in a row: a failed attempt ends the run, so the
  // misses after it are retried at once again rather than counted as
  // failures for the misses before it.
  it('starts a new run of misses after a failed standby attempt', () => {
    const miss = replicas(R1);
    const steps = [...Array<ReplicasMessage>(STANDBY_MISSES).fill(miss), 'drop' as const, ...Array<ReplicasMessage>(STANDBY_MISSES).fill(miss)];
    const t = standbyLandings(steps);
    expect(t.retried).toBe(steps.length);
    const s = t.sockets.at(-1)!;
    s.open();
    s.deliver(replicas(R2));
    s.deliver(resume(R2, 1));
    expect(t.engine.standbyState).toBe('Ready');
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

  it('counts no stall of its own as silence of the peers: it yields to the messages queued meanwhile once', () => {
    const { clock, engine, primary, standby } = live();
    beats(clock, 500, [primary, R1], [standby, R2]);
    clock.stall(2000);
    // The pings the replicas sent during the stall run after the late timers.
    primary.deliver({ type: 'ping', replica: R1, t: 1 });
    standby.deliver({ type: 'ping', replica: R2, t: 1 });
    clock.advance(0);
    expect(primary.closed).toBe(false);
    expect(standby.closed).toBe(false);
    expect(standby.ofType('promote')).toHaveLength(0);
    beats(clock, 2000, [primary, R1], [standby, R2]);
    expect(engine.link).toBe('Live');
    expect(engine.standbyState).toBe('Ready');
  });

  it('still fails over after a stall when the primary sent nothing meanwhile', () => {
    const { clock, engine, primary, standby } = live();
    beats(clock, 500, [primary, R1], [standby, R2]);
    clock.stall(2000);
    standby.deliver({ type: 'ping', replica: R2, t: 1 });
    clock.advance(0);
    expect(primary.closed).toBe(true);
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(engine.status().failover?.reason).toBe('dead');
  });

  it('drops no command for its expiry over a stall whose answer was queued meanwhile', () => {
    const { clock, engine, primary, standby, tick } = live();
    tick();
    engine.input({ type: 'set', path: '/channels/ch1/mute', value: true });
    const [set] = primary.ofType('set');
    clock.stall(COMMAND_EXPIRY_MS + 500);
    primary.deliver(delta(R1, 11, { changes: [{ path: '/channels/ch1/mute', value: true }], origins: [{ owner: YOU, commandId: set!.id, seq: set!.seq }] }));
    standby.deliver({ type: 'ping', replica: R2, t: 1 });
    clock.advance(0);
    expect(engine.outbox.size).toBe(0);
    expect(tick()!.notices).toBeUndefined();
  });

  it('leaves no primary for its sync timeout over a stall whose snapshot was queued meanwhile', () => {
    const t = setup();
    const s = t.sockets[0]!;
    s.open();
    t.clock.stall(SYNC_TIMEOUT_MS + 500);
    s.deliver(replicas(R1));
    s.deliver(snapshot(R1, 4));
    t.clock.advance(0);
    expect(s.closed).toBe(false);
    expect(t.engine.link).toBe('Live');
  });

  it('abandons no socket for its connect timeout over a stall whose open was queued meanwhile', () => {
    const t = setup();
    const s = t.sockets[0]!;
    t.clock.stall(FIRST_CONNECT_TIMEOUT_MS + 500);
    s.open();
    t.clock.advance(0);
    expect(s.closed).toBe(false);
    expect(t.engine.link).toBe('Syncing');
    expect(s.sent[0]).toMatchObject({ type: 'hello', mode: 'primary' });
  });

  it('fails over after three missed beats', () => {
    const { clock, engine, primary, standby, tick } = live();
    beats(clock, DEAD_MS - 50, [standby, R2]);
    expect(engine.link).toBe('Suspect');
    expect(standby.ofType('promote')).toHaveLength(0);
    beats(clock, 100, [standby, R2]);
    expect(primary.closed).toBe(true);
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(standby.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 10, epoch: EPOCH }]);
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

  it('names the build of the primary replica, and after a failover that of the promoted one', () => {
    const t = setup();
    expect(t.engine.status().build).toBeNull();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 10));
    const standby = t.sockets[1]!;
    standby.open();
    standby.deliver(replicas(R2));
    standby.deliver(resume(R2, 10));
    // The standby's replica said its build: the status still names the primary's.
    expect(t.engine.status().build).toEqual({ faderz: '0.4.1', samplez: '0.12.3' });
    t.sockets[0]!.drop();
    standby.deliver(resume(R2, 10));
    expect(t.engine.status()).toMatchObject({ primaryReplica: 'r2', build: { faderz: '0.4.2', samplez: '0.12.3' } });
  });

  it('fails over when the serving replica reports it cannot commit and the standby still can a moment later', () => {
    const { clock, engine, primary, standby } = live();
    primary.deliver(replicas(R1, ['r1']));
    expect(primary.closed).toBe(false);
    beats(clock, COMMIT_SETTLE_MS, [primary, R1], [standby, R2]);
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
    beats(t.clock, COMMIT_SETTLE_MS, [t.sockets[0]!, R1], [sb, R2]);
    expect(t.sockets[0]!.closed).toBe(true);
    expect(sb.ofType('promote')).toHaveLength(1);
  });

  it('stays on its primary and keeps its standby while no replica can commit, and fails over once the standby can', () => {
    const { clock, engine, primary, standby, sockets } = live();
    const all = ['r1', 'r2', 'r3'];
    primary.deliver(replicas(R1, all));
    standby.deliver(replicas(R2, all));
    beats(clock, 10_000, [primary, R1], [standby, R2]);
    expect(primary.closed).toBe(false);
    expect(standby.closed).toBe(false);
    expect(sockets).toHaveLength(2);
    expect(standby.ofType('promote')).toEqual([]);
    expect(engine.status()).toMatchObject({ link: 'Live', primaryReplica: 'r1', standby: 'Ready', standbyReplica: 'r2', failover: null });
    standby.deliver(replicas(R2, ['r1', 'r3']));
    beats(clock, COMMIT_SETTLE_MS, [primary, R1], [standby, R2]);
    expect(standby.ofType('promote')).toHaveLength(1);
    expect(engine.status().failover).toMatchObject({ from: 'r1', to: 'r2', reason: 'notCommitable' });
  });

  it('looks for a third replica that can commit when neither the primary nor the standby can, and fails over to it', () => {
    const { clock, engine, primary, standby, sockets } = live(['ws://a/ws', 'ws://b/ws', 'ws://c/ws']);
    // r1 and r2 cannot commit; their reports say r3 can.
    primary.deliver(replicas(R1, ['r1', 'r2']));
    standby.deliver(replicas(R2, ['r1', 'r2']));
    expect(standby.closed).toBe(false);
    beats(clock, COMMIT_SETTLE_MS, [primary, R1], [standby, R2]);
    expect(standby.closed).toBe(true);
    expect(primary.closed).toBe(false);
    clock.advance(STANDBY_RETRY_MS);
    const next = sockets.at(-1)!;
    expect(next).not.toBe(standby);
    next.open();
    next.deliver(replicas(R3, ['r1', 'r2']));
    next.deliver(resume(R3, 10));
    beats(clock, COMMIT_SETTLE_MS, [primary, R1], [next, R3]);
    expect(next.ofType('promote')).toHaveLength(1);
    expect(engine.status().failover).toMatchObject({ from: 'r1', to: 'r3', reason: 'notCommitable' });
  });

  it('fails nothing over onto a standby whose own report of losing commit arrives just after the primary one', () => {
    const { clock, primary, standby } = live();
    const all = ['r1', 'r2', 'r3'];
    primary.deliver(replicas(R1, all));
    clock.advance(COMMIT_SETTLE_MS / 2);
    standby.deliver(replicas(R2, all));
    beats(clock, 2000, [primary, R1], [standby, R2]);
    expect(primary.closed).toBe(false);
    expect(standby.closed).toBe(false);
    expect(standby.ofType('promote')).toEqual([]);
  });

  it('keeps its standby when the standby hears first that no replica can commit', () => {
    const { clock, primary, standby } = live();
    const all = ['r1', 'r2', 'r3'];
    standby.deliver(replicas(R2, all));
    clock.advance(COMMIT_SETTLE_MS / 2);
    primary.deliver(replicas(R1, all));
    beats(clock, 2000, [primary, R1], [standby, R2]);
    expect(standby.closed).toBe(false);
    expect(primary.closed).toBe(false);
  });

  it('lets a standby that cannot commit go once the primary can again, to look for one that can', () => {
    const { clock, primary, standby, sockets } = live();
    const all = ['r1', 'r2', 'r3'];
    primary.deliver(replicas(R1, all));
    standby.deliver(replicas(R2, all));
    expect(standby.closed).toBe(false);
    primary.deliver(replicas(R1, ['r2']));
    expect(standby.closed).toBe(false);
    beats(clock, COMMIT_SETTLE_MS, [primary, R1], [standby, R2]);
    expect(standby.closed).toBe(true);
    clock.advance(STANDBY_RETRY_MS);
    expect(sockets).toHaveLength(3);
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
    const again = t.sockets.find((s, i) => i > 0 && s.url === `ws://b/ws?session=${SESSION}` && s.onopen)!;
    again.open();
    expect(again.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'primary', lastRevision: 3, epoch: EPOCH });
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
    // A revision gap: the resync is answered with a snapshot.
    primary.deliver(delta(R1, 12, {}));
    expect(primary.ofType('resync')).toHaveLength(1);
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

  it('resends every sent command after a snapshot answering no request', () => {
    // The server sends a snapshot in place of deltas when its queue for
    // this client fills; the deltas it skips may have answered any sent
    // command.
    const { engine, primary } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = primary.ofType('set');
    expect(engine.link).toBe('Live');
    primary.deliver(snapshot(R1, 15));
    expect(primary.ofType('set').map((m) => m.id)).toEqual([set!.id, set!.id]);
    expect(engine.outbox.inFlight).toBe(1);
  });

  it('resends after a snapshot only commands sent before the resync it answers', () => {
    // A command sent after the resync is read after it and answered after
    // its snapshot: a copy would hold a second place in the server's
    // 64-command read window that the client never counts.
    const { engine, primary } = live();
    engine.input({ type: 'set', path: F1, value: -5 });
    primary.deliver(delta(R1, 12, {}));
    expect(engine.link).toBe('Resyncing');
    engine.input({ type: 'set', path: F2, value: -6 });
    const [before, after] = primary.ofType('set');
    expect(after!.path).toBe(F2);
    primary.deliver(snapshot(R1, 15));
    const sets = primary.ofType('set');
    expect(sets.map((m) => m.id)).toEqual([before!.id, after!.id, before!.id]);
    expect(engine.outbox.inFlight).toBe(2);
  });

  it('counts no standby hello as a request a snapshot answers', () => {
    // After a promote the engine sends a resync and opens a new standby,
    // whose hello the primary's snapshot does not answer: a command sent
    // between the two is read after the resync and is not sent again.
    const { engine, sockets, standby } = live();
    sockets[0]!.drop();
    expect(standby.ofType('resync')).toHaveLength(1);
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = standby.ofType('set');
    const next = sockets[2]!;
    next.open();
    expect(next.sent[0]).toMatchObject({ type: 'hello', mode: 'standby' });
    standby.deliver(snapshot(R2, 11));
    expect(engine.link).toBe('Live');
    expect(standby.ofType('set').map((m) => m.id)).toEqual([set!.id]);
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

  it('orders touch, set and release of a control on one seq counter and resends a touch with its seq', () => {
    const { engine, primary, standby } = live();
    engine.input({ type: 'touch', path: F1 });
    engine.input({ type: 'set', path: F1, value: -5 });
    engine.input({ type: 'release', path: F1 });
    engine.input({ type: 'touch', path: F2 });
    const sent = primary.sent.filter((m) => m.type === 'touch' || m.type === 'set' || m.type === 'release');
    expect(sent.map((m) => [m.type, m.path, (m as { seq: number }).seq])).toEqual([
      ['touch', F1, SEQ_BASE + 1],
      ['set', F1, SEQ_BASE + 2],
      ['release', F1, SEQ_BASE + 3],
      ['touch', F2, SEQ_BASE + 1],
    ]);
    primary.drop();
    const resent = standby.sent.filter((m) => m.type === 'touch' || m.type === 'set' || m.type === 'release');
    expect(resent).toEqual(sent);
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

  it('sends discrete writes of one control within 20 ms each at once, in order, with rising seq', () => {
    const { engine, primary, clock } = live();
    const MUTE = '/channels/ch1/mute';
    for (const value of [true, false, true]) {
      engine.input({ type: 'set', path: MUTE, value });
      clock.advance(5);
    }
    const sets = primary.ofType('set').filter((m) => m.path === MUTE);
    expect(sets.map((m) => m.value)).toEqual([true, false, true]);
    expect(sets[1]!.seq).toBeGreaterThan(sets[0]!.seq);
    expect(sets[2]!.seq).toBeGreaterThan(sets[1]!.seq);
    expect(new Set(sets.map((m) => m.id)).size).toBe(3);
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

describe('sync engine: unanswered commands', () => {
  const MUTE = '/channels/ch1/mute';

  it('never sends a command without an answer again on its connection, and drops it at its expiry so its control shows the committed value', () => {
    const { engine, primary, standby, clock, tick } = live();
    tick();
    engine.input({ type: 'set', path: MUTE, value: true });
    const [set] = primary.ofType('set');
    primary.deliver(delta(R1, 11, { changes: [{ path: MUTE, value: false }, { path: F2, value: -3 }] }));
    beats(clock, COMMAND_EXPIRY_MS - HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(primary.ofType('set')).toEqual([set]);
    expect(engine.outbox.inFlight).toBe(1);
    // Held back while the write is unanswered.
    expect(tick()!.changes).toEqual([[F2, -3]]);
    beats(clock, HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(engine.outbox.size).toBe(0);
    const p = tick()!;
    expect(p.changes).toEqual([[MUTE, false]]);
    expect(p.notices).toEqual([{ kind: 'expired', paths: [MUTE] }]);
    expect(p.status?.outbox).toBe(0);
    beats(clock, 5000, [primary, R1], [standby, R2]);
    expect(primary.ofType('set')).toEqual([set]);
  });

  it('counts every unanswered command in flight until it is answered or dropped, so a held connection gets no more than 64', () => {
    // A server holding its answers (a promote onto a lagging replica) reads
    // at most 64 commands ahead; copies would fill that window and push the
    // heartbeats and the resync behind them.
    const { engine, primary, standby, clock } = live();
    for (let i = 0; i < 100; i++) engine.input({ type: 'set', path: `/channels/ch${(i % 20) + 1}/mute`, value: i % 2 === 0 });
    beats(clock, COMMAND_EXPIRY_MS - HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(primary.ofType('set')).toHaveLength(MAX_COMMANDS_IN_FLIGHT);
    expect(engine.outbox.inFlight).toBe(MAX_COMMANDS_IN_FLIGHT);
  });

  it('sends a command answered not applied again with its id, after a pause', () => {
    const { engine, primary, standby, clock } = live();
    engine.input({ type: 'set', path: MUTE, value: true });
    const [set] = primary.ofType('set');
    primary.deliver({ type: 'error', replica: R1, code: 503, message: 'not applied', commandId: set!.id, path: MUTE });
    // A command sent meanwhile does not take the held one along.
    engine.input({ type: 'set', path: '/channels/ch2/mute', value: true });
    expect(primary.ofType('set').filter((m) => m.id === set!.id)).toHaveLength(1);
    expect(engine.outbox.size).toBe(2);
    beats(clock, NOT_APPLIED_RETRY_MS + HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(primary.ofType('set').filter((m) => m.id === set!.id)).toEqual([set, set]);
    primary.deliver(delta(R1, 11, { changes: [{ path: MUTE, value: true }], origins: [{ owner: YOU, commandId: set!.id, seq: set!.seq }] }));
    expect(engine.outbox.size).toBe(1);
  });

  it('touches again when its touch, resent after not applied, is refused as stale behind a later set', () => {
    const { engine, primary, standby, clock } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver({ type: 'error', replica: R1, code: 503, message: 'not applied', commandId: touch!.id, path: F1 });
    engine.input({ type: 'set', path: F1, value: -5 });
    const [set] = primary.ofType('set');
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -5 }], origins: [{ owner: YOU, commandId: set!.id, seq: set!.seq }] }));
    beats(clock, NOT_APPLIED_RETRY_MS + HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(primary.ofType('touch')).toEqual([touch, touch]);
    primary.deliver({ type: 'ack', replica: R1, commandId: touch!.id, result: 'stale', revision: 11 });
    // Still holding the fader and owning nothing: a new touch above the set.
    expect(engine.touches.get(F1)).toBe('Touching');
    const fresh = primary.ofType('touch').filter((m) => m.id !== touch!.id);
    expect(fresh).toHaveLength(1);
    expect(fresh[0]!.seq).toBeGreaterThan(set!.seq);
  });

  it('releases again when its release is answered stale while the session still owns the control', () => {
    const { engine, primary } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver(delta(R1, 11, { owners: [{ path: F1, owner: YOU }], origins: [{ owner: YOU, commandId: touch!.id, seq: touch!.seq }] }));
    engine.input({ type: 'release', path: F1 });
    const [release] = primary.ofType('release');
    primary.deliver({ type: 'ack', replica: R1, commandId: release!.id, result: 'stale', revision: 11 });
    expect(engine.touches.has(F1)).toBe(false);
    const fresh = primary.ofType('release').filter((m) => m.id !== release!.id);
    expect(fresh).toHaveLength(1);
    expect(fresh[0]!.seq).toBeGreaterThan(release!.seq);
  });

  it('keeps at most 64 commands in flight on a connection and sends the next as answers come', () => {
    const { engine, primary } = live();
    for (let i = 0; i < 100; i++) engine.input({ type: 'set', path: `/channels/ch${(i % 20) + 1}/mute`, value: i % 2 === 0 });
    const sent = primary.ofType('set');
    expect(sent).toHaveLength(MAX_COMMANDS_IN_FLIGHT);
    primary.deliver({ type: 'ack', replica: R1, commandId: sent[0]!.id, result: 'noop', revision: 10 });
    primary.deliver({ type: 'ack', replica: R1, commandId: sent[1]!.id, result: 'noop', revision: 10 });
    expect(primary.ofType('set')).toHaveLength(MAX_COMMANDS_IN_FLIGHT + 2);
    expect(engine.outbox.inFlight).toBe(MAX_COMMANDS_IN_FLIGHT);
  });

  it('touches again with a new command when an unanswered touch expires while still touching', () => {
    const { engine, primary, standby, clock, tick } = live();
    tick();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    beats(clock, COMMAND_EXPIRY_MS + HEARTBEAT_MS, [primary, R1], [standby, R2]);
    const touches = primary.ofType('touch');
    const fresh = touches.filter((m) => m.id !== touch!.id);
    expect(fresh).toHaveLength(1);
    expect(fresh[0]!.seq).toBeGreaterThan(touch!.seq);
    expect(engine.touches.get(F1)).toBe('Touching');
    // The expired notice names writes not confirmed; a touch is no write.
    expect(tick()?.notices).toBeUndefined();
  });

  it('ends the touch of a control whose release expires unanswered, and releases again while it still owns it', () => {
    const { engine, primary, standby, clock } = live();
    engine.input({ type: 'touch', path: F1 });
    const [touch] = primary.ofType('touch');
    primary.deliver(delta(R1, 11, { owners: [{ path: F1, owner: YOU }], origins: [{ owner: YOU, commandId: touch!.id, seq: touch!.seq }] }));
    engine.input({ type: 'release', path: F1 });
    const [release] = primary.ofType('release');
    expect(engine.touches.get(F1)).toBe('Releasing');
    beats(clock, COMMAND_EXPIRY_MS + HEARTBEAT_MS, [primary, R1], [standby, R2]);
    expect(engine.touches.has(F1)).toBe(false);
    const fresh = primary.ofType('release').filter((m) => m.id !== release!.id);
    expect(fresh).toHaveLength(1);
    expect(fresh[0]!.seq).toBeGreaterThan(release!.seq);
  });
});

describe('sync engine: offline', () => {
  it('drops writes queued while every endpoint is down once they outlive the failover bound, and never sends them', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    t.tick();
    t.sockets[0]!.drop();
    t.engine.input({ type: 'set', path: '/channels/ch1/mute', value: true });
    t.engine.input({ type: 'set', path: F1, value: -4 });
    // Every attempt fails until the link is Offline and well past the bound.
    for (let i = 0; i < 20; i++) {
      t.clock.advance(500);
      for (const s of t.sockets) if (s.onclose && !s.closed) s.drop();
    }
    expect(t.engine.link === 'Offline' || t.engine.link === 'Connecting').toBe(true);
    expect(t.engine.outbox.size).toBe(0);
    const p = t.tick()!;
    expect(p.notices).toContainEqual({ kind: 'expired', paths: ['/channels/ch1/mute', F1] });
    const before = t.sockets.length;
    // The Offline retry has backed off, at most to its cap.
    t.clock.advance(RETRY_CAP_MS);
    const s = t.sockets.slice(before).find((x) => x.onopen && !x.closed)!;
    s.open();
    s.deliver(replicas(R2));
    s.deliver(resume(R2, 1));
    expect(t.engine.link).toBe('Live');
    expect(s.ofType('set')).toEqual([]);
  });
});

describe('sync engine: expiry between sweeps', () => {
  it('never sends a queued command older than its expiry when the link comes back between two sweeps', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    t.sockets[0]!.drop();
    t.clock.advance(100);
    t.engine.input({ type: 'set', path: '/channels/ch1/mute', value: true });
    // Back 3050 ms after the set, before the sweep at 3250 drops it.
    t.clock.advance(3050);
    const s = t.sockets.filter((x) => x.onopen && !x.closed).at(-1)!;
    s.open();
    s.deliver(replicas(R2));
    s.deliver(resume(R2, 1));
    expect(t.engine.link).toBe('Live');
    expect(s.ofType('set')).toEqual([]);
    t.clock.advance(HEARTBEAT_MS);
    expect(t.engine.outbox.size).toBe(0);
    expect(t.tick()!.notices).toContainEqual({ kind: 'expired', paths: ['/channels/ch1/mute'] });
    expect(s.ofType('set')).toEqual([]);
  });
});

describe('sync engine: read index', () => {
  it('sends its revision on promote and resync and never applies an older snapshot or resume', () => {
    const { engine, primary, standby, clock } = live();
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -3 }] }));
    primary.drop();
    expect(standby.ofType('promote')).toEqual([{ type: 'promote', lastRevision: 11, epoch: EPOCH }]);
    expect(standby.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 11, epoch: EPOCH }]);
    standby.deliver(snapshot(R2, 10));
    standby.deliver(resume(R2, 10));
    expect(engine.revision).toBe(11);
    expect(getAt(engine.state, F1)).toBe(-3);
    expect(engine.link).toBe('Resyncing');
    clock.advance(RESYNC_RETRY_MS);
    expect(standby.ofType('resync')).toHaveLength(2);
    expect(standby.ofType('resync')[1]).toEqual({ type: 'resync', lastRevision: 11, epoch: EPOCH });
    standby.deliver(resume(R2, 11));
    expect(engine.link).toBe('Live');
    expect(getAt(engine.state, F1)).toBe(-3);
  });
});

describe('sync engine: answers that never come', () => {
  it('leaves a primary that answers no hello within the sync timeout for the next endpoint', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    t.sockets[0]!.drop();
    t.clock.advance(1000);
    const again = t.sockets.find((s, i) => i > 0 && s.onopen && !s.closed)!;
    again.open();
    expect(t.engine.link).toBe('Syncing');
    // Heartbeats flow, so only the missing answer can end the wait.
    beats(t.clock, SYNC_TIMEOUT_MS - HEARTBEAT_MS, [again, R2]);
    expect(again.closed).toBe(false);
    beats(t.clock, 2 * HEARTBEAT_MS, [again, R2]);
    expect(again.closed).toBe(true);
    expect(t.engine.link).toBe('Connecting');
  });

  it('passes over a promoted standby that answers only with stale older state, and applies none of it', () => {
    const { engine, primary, standby, clock, sockets } = live(['ws://a/ws', 'ws://b/ws', 'ws://c/ws']);
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -3 }] }));
    primary.drop();
    expect(engine.link).toBe('Resyncing');
    const stale = { ...snapshot(R2, 10), stale: true as const };
    for (let left = SYNC_TIMEOUT_MS - HEARTBEAT_MS; left > 0; left -= RESYNC_RETRY_MS) {
      standby.deliver(stale);
      beats(clock, RESYNC_RETRY_MS, [standby, R2]);
    }
    expect(engine.revision).toBe(11);
    expect(getAt(engine.state, F1)).toBe(-3);
    expect(standby.closed).toBe(false);
    beats(clock, 2 * HEARTBEAT_MS, [standby, R2]);
    expect(standby.closed).toBe(true);
    expect(engine.status().failover).toMatchObject({ n: 2, reason: 'noAnswer' });
    expect(sockets.at(-1)).not.toBe(standby);
  });

  it('applies a stale answer that is not older than its own state', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1, ['r1', 'r2', 'r3']));
    t.sockets[0]!.deliver({ ...snapshot(R1, 4), stale: true });
    expect(t.engine.link).toBe('Live');
    expect(t.engine.revision).toBe(4);
  });
});

describe('sync engine: ahead of the cluster', () => {
  it('stops in Ahead without retrying when the replica says this client is ahead, and takes the cluster state on adopt', () => {
    const { engine, primary, standby, sockets, clock, tick } = live();
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -3 }] }));
    primary.drop();
    expect(standby.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 11, epoch: EPOCH }]);
    standby.deliver({ type: 'ahead', replica: R2, epoch: EPOCH, revision: 7, lastRevision: 11 });
    expect(engine.link).toBe('Ahead');
    expect(tick()!.status?.ahead).toEqual({ lastRevision: 11, cluster: 7, otherHistory: false });
    beats(clock, 5 * RESYNC_RETRY_MS, [standby, R2]);
    expect(engine.link).toBe('Ahead');
    expect(standby.ofType('resync')).toHaveLength(1);
    engine.input({ type: 'adopt' });
    expect(standby.ofType('resync').at(-1)).toEqual({ type: 'resync' });
    standby.deliver(snapshot(R2, 7));
    expect(engine.link).toBe('Live');
    expect(engine.revision).toBe(7);
    expect(getAt(engine.state, F1)).not.toBe(-3);
    expect(tick()!.status?.ahead).toBeNull();
    // The standby opened during the failover is replaced by a new one.
    const next = sockets.at(-1)!;
    next.open();
    expect(next.sent[0]).toMatchObject({ type: 'hello', mode: 'standby', lastRevision: 7, epoch: EPOCH });
    expect(engine.standbyState).toBe('Syncing');
  });

  it('goes Ahead on a snapshot of another epoch even above its revision, applies none of its deltas and takes it on adopt', () => {
    // A cluster restored from a backup that has since committed past this
    // client: the revision numbers alone would let it resume onto the new
    // history.
    const { engine, primary, standby, sockets, clock, tick } = live();
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -3 }] }));
    primary.drop();
    expect(standby.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 11, epoch: EPOCH }]);
    standby.deliver(snapshot(R2, 15, {}, YOU, RESTORED));
    expect(engine.link).toBe('Ahead');
    expect(tick()!.status?.ahead).toEqual({ lastRevision: 11, cluster: 15, otherHistory: true });
    standby.deliver(delta(R2, 12, { changes: [{ path: F1, value: -40 }] }));
    expect(engine.revision).toBe(11);
    expect(engine.epoch).toBe(EPOCH);
    expect(getAt(engine.state, F1)).toBe(-3);
    beats(clock, 5 * RESYNC_RETRY_MS, [standby, R2]);
    expect(engine.link).toBe('Ahead');
    expect(standby.ofType('resync')).toHaveLength(1);
    engine.input({ type: 'adopt' });
    expect(standby.ofType('resync').at(-1)).toEqual({ type: 'resync' });
    standby.deliver(snapshot(R2, 16, {}, YOU, RESTORED));
    expect(engine.link).toBe('Live');
    expect(engine.revision).toBe(16);
    expect(engine.epoch).toBe(RESTORED);
    expect(getAt(engine.state, F1)).not.toBe(-3);
    const next = sockets.at(-1)!;
    expect(next).not.toBe(standby);
    next.open();
    expect(next.sent[0]).toEqual({ type: 'hello', session: SESSION, kind: 'ui', mode: 'standby', lastRevision: 16, epoch: RESTORED });
  });

  it('reconnects with its epoch and goes Ahead on a snapshot of another one', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    t.sockets[0]!.drop();
    t.clock.advance(1000);
    const again = t.sockets.find((s, i) => i > 0 && s.onopen)!;
    again.open();
    expect(again.sent[0]).toMatchObject({ type: 'hello', mode: 'primary', lastRevision: 3, epoch: EPOCH });
    again.deliver(snapshot(R1, 2, {}, YOU, RESTORED));
    expect(t.engine.link).toBe('Ahead');
    expect(t.engine.revision).toBe(3);
  });

  it('never applies a resume of another epoch and asks for a snapshot instead', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    t.sockets[0]!.drop();
    t.clock.advance(1000);
    const again = t.sockets.find((s, i) => i > 0 && s.onopen)!;
    again.open();
    again.deliver(resume(R1, 4, [delta(R1, 4, { changes: [{ path: F1, value: -40 }] })], RESTORED));
    expect(t.engine.revision).toBe(3);
    expect(getAt(t.engine.state, F1)).not.toBe(-40);
    expect(again.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 3, epoch: EPOCH, full: true }]);
  });

  it('drops a standby that syncs on another epoch, so its deltas never reach this state', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    const sb = t.sockets[1]!;
    sb.open();
    sb.deliver(replicas(R2));
    sb.deliver(resume(R2, 4, [delta(R2, 4, { changes: [{ path: F1, value: -40 }] })], RESTORED));
    expect(sb.closed).toBe(true);
    expect(t.engine.standbyState).toBe('Idle');
    expect(t.engine.revision).toBe(3);
  });

  it('goes Ahead from Live on a snapshot of another epoch on the open primary, and applies no delta after it', () => {
    // A server that does not close its connections on an epoch change, as
    // the contract has it, sends a snapshot of the new history on an open
    // one; its deltas follow.
    const { engine, primary, standby, tick } = live();
    primary.deliver(snapshot(R1, 15, {}, YOU, RESTORED));
    expect(engine.link).toBe('Ahead');
    expect(tick()!.status?.ahead).toEqual({ lastRevision: 10, cluster: 15, otherHistory: true });
    expect(standby.closed).toBe(true);
    primary.deliver(delta(R1, 11, { changes: [{ path: F1, value: -40 }] }));
    primary.deliver(delta(R1, 16, { changes: [{ path: F1, value: -41 }] }));
    expect(engine.revision).toBe(10);
    expect(engine.epoch).toBe(EPOCH);
    expect(getAt(engine.state, F1)).not.toBe(-40);
    expect(primary.ofType('resync')).toHaveLength(0);
  });

  it('goes Ahead from Suspect too', () => {
    const { clock, engine, primary, standby } = live();
    clock.advance(SUSPECT_MS + 10);
    standby.deliver({ type: 'ping', replica: R2, t: 1 });
    expect(engine.link).toBe('Suspect');
    primary.deliver(snapshot(R1, 15, {}, YOU, RESTORED));
    expect(engine.link).toBe('Ahead');
  });

  it('drops a standby whose snapshot is of another epoch, so a delta from it never applies', () => {
    const t = setup();
    t.sockets[0]!.open();
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 3));
    const sb = t.sockets[1]!;
    sb.open();
    sb.deliver(replicas(R2));
    sb.deliver(snapshot(R2, 3, {}, YOU, RESTORED));
    expect(sb.closed).toBe(true);
    expect(t.engine.standbyState).toBe('Idle');
    sb.deliver(delta(R2, 4, { changes: [{ path: F1, value: -40 }] }));
    expect(t.engine.revision).toBe(3);
    expect(getAt(t.engine.state, F1)).not.toBe(-40);
  });

  it('closes the standby when the replica answers ahead', () => {
    const { engine, primary, standby } = live();
    primary.deliver({ type: 'ahead', replica: R1, epoch: EPOCH, revision: 7, lastRevision: 10 });
    expect(engine.link).toBe('Ahead');
    expect(standby.closed).toBe(true);
    expect(engine.standbyState).toBe('Idle');
  });

  it('leaves a primary that answers no snapshot within the sync timeout after adopt', () => {
    const { engine, primary, clock } = live();
    primary.deliver({ type: 'ahead', replica: R1, epoch: EPOCH, revision: 7, lastRevision: 10 });
    engine.input({ type: 'adopt' });
    expect(engine.link).toBe('Resyncing');
    beats(clock, SYNC_TIMEOUT_MS - HEARTBEAT_MS, [primary, R1]);
    expect(primary.closed).toBe(false);
    beats(clock, 2 * HEARTBEAT_MS, [primary, R1]);
    expect(primary.closed).toBe(true);
    expect(engine.status().failover).toMatchObject({ reason: 'noAnswer' });
  });

  it('ignores adopt while not ahead', () => {
    const { engine, primary } = live();
    engine.input({ type: 'adopt' });
    expect(primary.ofType('resync')).toHaveLength(0);
    expect(engine.link).toBe('Live');
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

describe('sync engine: selections and control requests', () => {
  it('sends the selection on the primary only, again on every new primary, and posts the selections either connection reports', () => {
    const { engine, primary, standby, tick } = live();
    engine.input({ type: 'select', element: 'ch3' });
    engine.input({ type: 'select', element: 'ch3' });
    // Once on going live, to clear what a reload left, then once per change.
    expect(primary.ofType('select')).toEqual([
      { type: 'select', element: null },
      { type: 'select', element: 'ch3' },
    ]);
    expect(standby.ofType('select')).toEqual([]);
    const sel = { session: OTHER, element: 'ch5' };
    primary.deliver({ type: 'selections', replica: R1, selections: [sel] });
    expect(tick()!.selections).toEqual([sel]);
    expect(tick()).toBeNull();
    primary.drop();
    standby.deliver(resume(R2, 10));
    expect(standby.ofType('select')).toEqual([{ type: 'select', element: 'ch3' }]);
    engine.input({ type: 'select', element: null });
    expect(standby.ofType('select').at(-1)).toEqual({ type: 'select', element: null });
  });

  it('sends a control request on a transmitting primary, says it did, and turns one for this session into a notice', () => {
    const { engine, primary, standby, tick } = live(undefined, { [F1]: OTHER });
    engine.input({ type: 'request', path: F1 });
    expect(primary.ofType('request')).toEqual([{ type: 'request', path: F1 }]);
    expect(standby.ofType('request')).toEqual([]);
    primary.deliver({ type: 'controlRequest', replica: R1, path: F2, from: OTHER });
    expect(tick()!.notices).toEqual([
      { kind: 'requested', path: F1, owner: OTHER },
      { kind: 'request', path: F2, from: OTHER },
    ]);
  });

  it('sends no control request and says nothing while the link transmits nothing', () => {
    const t = setup();
    t.engine.input({ type: 'request', path: F1 });
    t.sockets[0]!.open();
    expect(t.sockets[0]!.ofType('request')).toEqual([]);
    expect(t.tick()?.notices).toBeUndefined();
  });

  it('shows the selections of its primary only, and those its standby had once promoted', () => {
    const { primary, standby, tick } = live();
    const onPrimary = [{ session: OTHER, element: 'ch5' }];
    const onStandby = [{ session: OTHER, element: 'ch6' }];
    primary.deliver({ type: 'selections', replica: R1, selections: onPrimary });
    standby.deliver({ type: 'selections', replica: R2, selections: onStandby });
    expect(tick()!.selections).toEqual(onPrimary);
    standby.deliver({ type: 'selections', replica: R2, selections: onStandby });
    expect(tick()?.selections).toBeUndefined();
    primary.drop();
    expect(tick()!.selections).toEqual(onStandby);
  });

  it("shows the presence of its primary's replica only, and its standby's once promoted", () => {
    const { primary, standby, tick } = live();
    const at = '2026-11-17T09:00:00.000Z';
    const onPrimary = [{ handle: OTHER.handle, kind: 'ui' as const, user: 'alice', role: 'operator' as const, since: at, active: at }];
    const onStandby = [{ ...onPrimary[0]!, user: 'bob' }];
    primary.deliver({ type: 'presence', replica: R1, sessions: onPrimary });
    standby.deliver({ type: 'presence', replica: R2, sessions: onStandby });
    expect(tick()!.presence).toEqual(onPrimary);
    standby.deliver({ type: 'presence', replica: R2, sessions: onStandby });
    expect(tick()?.presence).toBeUndefined();
    primary.drop();
    expect(tick()!.presence).toEqual(onStandby);
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

  it('reports the keys with an unanswered write, not the continuous controls', () => {
    const { engine, primary } = live();
    engine.input({ type: 'set', path: '/channels/ch1/mute', value: true });
    engine.input({ type: 'set', path: F1, value: -3 });
    expect(engine.status().pendingKeys).toEqual(['/channels/ch1/mute']);
    const id = primary.ofType('set').find((m) => m.path === '/channels/ch1/mute')!.id;
    primary.deliver(delta(R1, 11, { changes: [{ path: '/channels/ch1/mute', value: true }], origins: [{ owner: YOU, commandId: id }] }));
    expect(engine.status().pendingKeys).toEqual([]);
  });

  it('posts nothing on a tick with nothing new', () => {
    const { tick } = live();
    expect(tick()).not.toBeNull();
    expect(tick()).toBeNull();
  });

  it('sends resync with lastRevision on a revision gap', () => {
    const { engine, primary } = live();
    primary.deliver(delta(R1, 12, { changes: [{ path: F1, value: 1 }] }));
    expect(primary.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 10, epoch: EPOCH }]);
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

  it('sends a full resync with its revision once on a layout fingerprint mismatch', () => {
    const { primary } = live();
    const other = deriveMeterLayout(exampleShape(3));
    primary.deliver(frame(1, other));
    primary.deliver(frame(2, other));
    expect(primary.ofType('resync')).toEqual([{ type: 'resync', lastRevision: 10, epoch: EPOCH, full: true }]);
  });

  it('sends no layout resync while a resync is unanswered, and sends it once live again', () => {
    // One request at a time: a snapshot then answers the last one sent,
    // which tells which commands it answers for.
    const { primary } = live();
    primary.deliver(delta(R1, 12, {}));
    const other = deriveMeterLayout(exampleShape(3));
    primary.deliver(frame(1, other));
    expect(primary.ofType('resync')).toHaveLength(1);
    primary.deliver(resume(R1, 12, [delta(R1, 11, {}), delta(R1, 12, {})]));
    primary.deliver(frame(2, other));
    expect(primary.ofType('resync').at(-1)).toEqual({ type: 'resync', lastRevision: 12, epoch: EPOCH, full: true });
    expect(primary.ofType('resync')).toHaveLength(2);
  });
});

describe('sync engine: slow and lossy links', () => {
  /** Advances in 50 ms steps and returns every link state seen. */
  function states(t: ReturnType<typeof setup>, ms: number): Set<string> {
    const seen = new Set<string>([t.engine.link]);
    for (let left = ms; left > 0; left -= 50) {
      t.clock.advance(Math.min(50, left));
      seen.add(t.engine.link);
    }
    return seen;
  }

  /** Pings of this client answered so far, per socket. */
  const answered = new WeakMap<FakeSocket, number>();

  /**
   * Heartbeat steps over a link with round trip `rtt`: the replicas' pings
   * arrive every period, and every ping of this client is answered `rtt`
   * after it was sent.
   */
  function slowBeats(clock: FakeClock, ms: number, rtt: number, ...peers: Array<[FakeSocket, ReplicaRef]>) {
    for (let left = ms; left > 0; left -= HEARTBEAT_MS) {
      clock.advance(Math.min(HEARTBEAT_MS, left));
      for (const [s, r] of peers) {
        s.deliver({ type: 'ping', replica: r, t: clock.now() });
        const pings = s.ofType('ping');
        let i = answered.get(s) ?? 0;
        for (; i < pings.length && pings[i]!.t <= clock.now() - rtt; i++) s.deliver({ type: 'pong', replica: r, t: pings[i]!.t });
        answered.set(s, i);
      }
    }
  }

  it('connects on the first attempt over a link whose socket opens after 6 s, and never shows Offline', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    const seen = states(t, 6000);
    expect(t.sockets).toHaveLength(1);
    expect(t.sockets[0]!.closed).toBe(false);
    expect([...seen]).toEqual(['Connecting']);
    t.sockets[0]!.open();
    expect(t.engine.link).toBe('Syncing');
    t.sockets[0]!.deliver(replicas(R1));
    t.sockets[0]!.deliver(snapshot(R1, 1));
    expect(t.engine.link).toBe('Live');
  });

  it('fails over at once from an endpoint that refuses, and waits for the next one to open slowly', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.sockets[0]!.drop();
    expect(t.engine.link).toBe('Connecting');
    t.clock.advance(PRIMARY_RETRY_MS[1]!);
    expect(t.sockets).toHaveLength(2);
    const b = t.sockets[1]!;
    expect(b.url).toBe(`ws://b/ws?session=${SESSION}`);
    const seen = states(t, 5000);
    expect(seen.has('Offline')).toBe(false);
    expect(b.closed).toBe(false);
    expect(t.sockets).toHaveLength(2);
    b.open();
    expect(t.engine.link).toBe('Syncing');
  });

  it('gives each socket in a row abandoned unopened twice the time, up to the cap', () => {
    const t = setup();
    t.clock.advance(FIRST_CONNECT_TIMEOUT_MS - 1);
    expect(t.sockets[0]!.closed).toBe(false);
    t.clock.advance(1);
    expect(t.sockets[0]!.closed).toBe(true);
    t.clock.advance(PRIMARY_RETRY_MS[1]!);
    const second = t.sockets[1]!;
    t.clock.advance(2 * FIRST_CONNECT_TIMEOUT_MS - 1);
    expect(second.closed).toBe(false);
    t.clock.advance(1);
    expect(second.closed).toBe(true);
    t.clock.advance(PRIMARY_RETRY_MS[2]!);
    const third = t.sockets[2]!;
    t.clock.advance(CONNECT_TIMEOUT_CAP_MS - 1);
    expect(third.closed).toBe(false);
    t.clock.advance(1);
    expect(third.closed).toBe(true);
    expect(t.engine.link).toBe('Offline');
  });

  it('backs off its Offline retries exponentially, jittered below the unjittered delay', () => {
    const t = setup(['ws://a/ws'], SEQ_BASE, () => 0.5);
    const delays: number[] = [];
    let offlineAt: number | null = null;
    let seen = 0;
    for (let i = 0; i < 6000 && delays.length < 5; i++) {
      // Every endpoint refuses at once.
      for (; seen < t.sockets.length; seen++) t.sockets[seen]!.drop();
      if (offlineAt !== null && t.engine.link === 'Connecting') {
        delays.push(t.clock.now() - offlineAt);
        offlineAt = null;
      }
      if (offlineAt === null && t.engine.link === 'Offline') offlineAt = t.clock.now();
      t.clock.advance(10);
    }
    expect(delays).toEqual([0.9 * OFFLINE_RETRY_MS, 0.9 * 2 * OFFLINE_RETRY_MS, 0.9 * 4 * OFFLINE_RETRY_MS, 0.9 * RETRY_CAP_MS, 0.9 * RETRY_CAP_MS]);
  });

  it('raises and lowers its deadlines with the measured round trip, within their floors and caps', () => {
    const t = setup();
    expect(t.engine.connectTimeout('primary')).toBe(FIRST_CONNECT_TIMEOUT_MS);
    const feed = (ms: number, n: number) => {
      for (let i = 0; i < n; i++) t.engine.input({ type: 'rtt', ms });
    };
    feed(1, 50);
    expect(t.engine.connectTimeout('primary')).toBe(CONNECT_TIMEOUT_MS);
    expect(t.engine.syncTimeout()).toBe(SYNC_TIMEOUT_MS);
    expect(t.engine.deadMs()).toBeLessThan(DEAD_MS + 5);
    expect(t.engine.suspectMs()).toBeLessThan(SUSPECT_MS + 5);

    feed(1500, 50);
    expect(t.engine.connectTimeout('primary')).toBeGreaterThanOrEqual(4 * 1500);
    expect(t.engine.connectTimeout('primary')).toBeLessThan(4 * 1550);
    expect(t.engine.syncTimeout()).toBeGreaterThanOrEqual(2000 + 2 * 1500);
    expect(t.engine.syncTimeout()).toBeLessThan(2000 + 2 * 1550);
    expect(t.engine.deadMs()).toBeGreaterThanOrEqual(DEAD_MS + 1500);
    expect(t.engine.deadMs()).toBeLessThan(DEAD_MS + 1550);

    feed(50_000, 50);
    expect(t.engine.connectTimeout('primary')).toBe(CONNECT_TIMEOUT_CAP_MS);
    expect(t.engine.syncTimeout()).toBe(SYNC_TIMEOUT_CAP_MS);
    expect(t.engine.deadMs()).toBe(DEAD_MS + SLACK_CAP_MS);

    feed(1, 100);
    expect(t.engine.connectTimeout('primary')).toBe(CONNECT_TIMEOUT_MS);
    expect(t.engine.syncTimeout()).toBe(SYNC_TIMEOUT_MS);
    expect(t.engine.deadMs()).toBeLessThan(DEAD_MS + 5);
  });

  it('measures the round trip only from pongs that answer a ping it sent on that connection, once each', () => {
    const { clock, engine, primary, standby } = live();
    beats(clock, 2000, [primary, R1], [standby, R2]);
    const sent = primary.ofType('ping').map((p) => p.t);
    const t = clock.now() - 1500;
    expect(sent).toContain(t);
    // No ping of this client carries this t.
    primary.deliver({ type: 'pong', replica: R1, t: t + 1 });
    expect(engine.rtt.srtt).toBeNull();
    primary.deliver({ type: 'pong', replica: R1, t });
    expect(engine.rtt.srtt).toBe(1500);
    // The same answer again, and an answer to an earlier ping it passed over.
    primary.deliver({ type: 'pong', replica: R1, t });
    primary.deliver({ type: 'pong', replica: R1, t: t - HEARTBEAT_MS });
    expect(engine.rtt.srtt).toBe(1500);
  });

  it('keeps a primary deadline at its floor while a silent standby endpoint is abandoned again and again', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.engine.input({ type: 'rtt', ms: 5 });
    const a = t.sockets[0]!;
    a.open();
    a.deliver(replicas(R1));
    a.deliver(snapshot(R1, 1));
    // The standby endpoint never answers: each attempt there is abandoned.
    for (let i = 0; i < 6; i++) {
      const sb = t.sockets.at(-1)!;
      expect(sb.url).toBe(`ws://b/ws?session=${SESSION}`);
      beats(t.clock, t.engine.connectTimeout('standby') + 30_000, [a, R1]);
      expect(sb.closed).toBe(true);
    }
    expect(t.engine.connectTimeout('standby')).toBe(CONNECT_TIMEOUT_CAP_MS);
    expect(t.engine.connectTimeout('primary')).toBe(CONNECT_TIMEOUT_MS);
    a.drop();
    t.clock.advance(PRIMARY_RETRY_MS[1]!);
    const b = t.sockets.find((s) => s.url === `ws://b/ws?session=${SESSION}` && !s.closed)!;
    t.clock.advance(CONNECT_TIMEOUT_MS - 1);
    expect(b.closed).toBe(false);
    t.clock.advance(1);
    expect(b.closed).toBe(true);
  });

  it('shortens the deadline of a first socket still opening once a round trip is measured, never below the floor', () => {
    const t = setup();
    const first = t.sockets[0]!;
    t.clock.advance(500);
    t.engine.input({ type: 'rtt', ms: 5 });
    t.clock.advance(CONNECT_TIMEOUT_MS - 500 - 1);
    expect(first.closed).toBe(false);
    t.clock.advance(1);
    expect(first.closed).toBe(true);

    // A slow measurement lengthens nothing past the first-connect deadline.
    const s = setup();
    s.engine.input({ type: 'rtt', ms: 5000 });
    s.clock.advance(FIRST_CONNECT_TIMEOUT_MS - 1);
    expect(s.sockets[0]!.closed).toBe(false);
    s.clock.advance(1);
    expect(s.sockets[0]!.closed).toBe(true);
  });

  it('doubles the sync timeout per primary in a row left without an answer, and resets it once one syncs', () => {
    const t = setup(['ws://a/ws', 'ws://b/ws']);
    t.engine.input({ type: 'rtt', ms: 5 });
    const silent = (n: number) => {
      const s = t.sockets.filter((x) => !x.closed).at(-1)!;
      s.open();
      const deadline = n === 0 ? SYNC_TIMEOUT_MS : 2 ** n * SYNC_TIMEOUT_MS;
      beats(t.clock, deadline - HEARTBEAT_MS, [s, R1]);
      expect(s.closed).toBe(false);
      beats(t.clock, 2 * HEARTBEAT_MS, [s, R1]);
      expect(s.closed).toBe(true);
      t.clock.advance(1000);
    };
    silent(0);
    silent(1);
    expect(t.engine.syncTimeout()).toBe(4 * SYNC_TIMEOUT_MS);
    const s = t.sockets.filter((x) => !x.closed).at(-1)!;
    s.open();
    s.deliver(replicas(R1));
    s.deliver(snapshot(R1, 1));
    expect(t.engine.link).toBe('Live');
    expect(t.engine.syncTimeout()).toBe(SYNC_TIMEOUT_MS);
  });

  it('starts its Offline backoff over once a primary syncs', () => {
    const t = setup(['ws://a/ws'], SEQ_BASE, () => 0);
    let seen = 0;
    const refuseUntilOffline = () => {
      for (let i = 0; i < 1000 && t.engine.link !== 'Offline'; i++) {
        for (; seen < t.sockets.length; seen++) t.sockets[seen]!.drop();
        t.clock.advance(10);
      }
      expect(t.engine.link).toBe('Offline');
    };
    const retryAfter = () => {
      const from = t.clock.now();
      while (t.engine.link === 'Offline') t.clock.advance(10);
      return t.clock.now() - from;
    };
    refuseUntilOffline();
    expect(retryAfter()).toBeLessThanOrEqual(OFFLINE_RETRY_MS);
    refuseUntilOffline();
    expect(retryAfter()).toBeGreaterThan(OFFLINE_RETRY_MS);
    const s = t.sockets.at(-1)!;
    seen = t.sockets.length;
    s.open();
    s.deliver(replicas(R1));
    s.deliver(snapshot(R1, 1));
    expect(t.engine.link).toBe('Live');
    s.drop();
    refuseUntilOffline();
    expect(retryAfter()).toBeLessThanOrEqual(OFFLINE_RETRY_MS);
  });

  it('keeps a live link over a 1.5 s round trip whose traffic stalls behind a retransmission', () => {
    const { clock, engine, primary, standby, tick } = live();
    slowBeats(clock, 5000, 1500, [primary, R1], [standby, R2]);
    expect(engine.link).toBe('Live');
    // A lost segment holds everything on the primary back: a TCP
    // retransmission timeout after the next ping was due.
    beats(clock, 1900, [standby, R2]);
    slowBeats(clock, 3000, 1500, [primary, R1], [standby, R2]);
    expect(primary.closed).toBe(false);
    expect(standby.ofType('promote')).toHaveLength(0);
    expect(engine.link).toBe('Live');
    expect(tick()!.status!.failover).toBeNull();
  });

  it('still fails over from a primary silent for far longer than the round trip', () => {
    const { clock, engine, primary, standby } = live();
    slowBeats(clock, 5000, 1500, [primary, R1], [standby, R2]);
    beats(clock, engine.deadMs() + HEARTBEAT_MS, [standby, R2]);
    expect(primary.closed).toBe(true);
    expect(standby.ofType('promote')).toHaveLength(1);
  });
});

describe('acknowledging commands', () => {
  // The page reads its outbox from the patches: a command that changes
  // nothing it shows, here a release of a control never touched, is
  // still answered by a patch carrying its input, or the page could not
  // tell its outbox covers it.
  it('answers a command that changes nothing shown with a patch', () => {
    const t = live();
    t.tick();
    expect(t.tick()).toBeNull();
    const before = t.patches.length;
    t.engine.input({ type: 'release', path: '/channels/ch1/fader' });
    expect(t.patches.length).toBe(before);
    const p = t.tick();
    expect(p).not.toBeNull();
    // The empty tick, the release and the tick after it.
    expect(p!.input).toBe(t.patches[before - 1]!.input + 3);
    expect(t.tick()).toBeNull();
  });
});
