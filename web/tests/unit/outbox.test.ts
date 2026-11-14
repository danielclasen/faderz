import { describe, expect, it } from 'vitest';
import { Outbox } from '../../src/lib/sync/outbox';
import type { CommandMessage } from '../../src/lib/contract/types';

const set = (id: string, path: string, value: unknown): CommandMessage => ({ type: 'set', id, seq: 1, path, value });
const touch = (id: string, path: string): CommandMessage => ({ type: 'touch', id, seq: 1, path });
const release = (id: string, path: string): CommandMessage => ({ type: 'release', id, seq: 1, path });
const ids = (msgs: CommandMessage[]) => msgs.map((m) => m.id);

describe('outbox', () => {
  it('coalesces queued sets of a continuous control to the latest value', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true);
    const r = o.enqueue(set('b', '/f', 2), true);
    o.enqueue(set('c', '/f', 3), true);
    expect(r).toEqual({ accepted: true, superseded: expect.objectContaining({ state: 'Superseded' }) });
    expect(ids(o.transmit())).toEqual(['c']);
  });

  it('keeps every discrete command in order', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/m', true), false);
    o.enqueue(set('b', '/m', false), false);
    o.enqueue(set('c', '/m', true), false);
    expect(ids(o.transmit())).toEqual(['a', 'b', 'c']);
  });

  it('never coalesces across a touch or release of the same control', () => {
    const o = new Outbox();
    o.enqueue(touch('t', '/f'), false);
    o.enqueue(set('a', '/f', 1), true);
    o.enqueue(release('r', '/f'), false);
    o.enqueue(set('b', '/f', 2), true);
    expect(ids(o.transmit())).toEqual(['t', 'a', 'r', 'b']);
  });

  it('never replaces a set that was sent', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true);
    o.transmit();
    o.enqueue(set('b', '/f', 2), true);
    expect(ids(o.transmit())).toEqual(['b']);
    expect(o.states()).toEqual([
      { id: 'a', state: 'Sent' },
      { id: 'b', state: 'Sent' },
    ]);
  });

  it('queues sent commands again on failover and sends each once more with the same id', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true);
    o.enqueue(touch('t', '/g'), false);
    o.transmit();
    expect(o.transmit()).toEqual([]);
    expect(o.failover()).toBe(2);
    expect(o.states().map((s) => s.state)).toEqual(['Queued', 'Queued']);
    expect(ids(o.transmit())).toEqual(['a', 't']);
    expect(o.transmit()).toEqual([]);
  });

  it('removes answered commands; unknown and repeated answers are ignored', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true);
    o.enqueue(set('b', '/g', 1), true);
    o.transmit();
    expect(o.answer('a', 'ack')?.state).toBe('Acked');
    expect(o.answer('a', 'ack')).toBeNull();
    expect(o.answer('zz', 'reject')).toBeNull();
    expect(o.answer('b', 'stale')?.state).toBe('Stale');
    expect(o.size).toBe(0);
    expect(o.hasSet('/f')).toBe(false);
  });

  it('refuses a new command when full and keeps what it holds', () => {
    const o = new Outbox(3);
    expect(o.enqueue(set('a', '/a', true), false).accepted).toBe(true);
    expect(o.enqueue(set('b', '/b', true), false).accepted).toBe(true);
    expect(o.enqueue(set('c', '/c', true), false).accepted).toBe(true);
    expect(o.enqueue(set('d', '/d', true), false)).toEqual({ accepted: false });
    expect(o.size).toBe(3);
    expect(o.hasSet('/d')).toBe(false);
    expect(ids(o.transmit())).toEqual(['a', 'b', 'c']);
  });

  it('still takes a newer value of a queued continuous control when full', () => {
    const o = new Outbox(2);
    o.enqueue(set('a', '/a', 1), true);
    o.enqueue(set('b', '/b', 1), true);
    expect(o.enqueue(set('c', '/b', 2), true).accepted).toBe(true);
    expect(ids(o.transmit())).toEqual(['a', 'c']);
  });

  it('keeps at most `limit` in flight, holds a not-applied command until its time and requeues overdue ones', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/a', 1), false, 0);
    o.enqueue(set('b', '/b', 1), false, 0);
    o.enqueue(set('c', '/c', 1), false, 0);
    expect(ids(o.transmit(10, 2))).toEqual(['a', 'b']);
    expect(o.inFlight).toBe(2);
    o.notApplied('a', 300);
    expect(ids(o.transmit(20, 2))).toEqual(['c']);
    expect(o.transmit(299, 2)).toEqual([]);
    expect(o.overdue(9)).toBe(0);
    expect(o.overdue(10)).toBe(1);
    expect(ids(o.transmit(300, 3))).toEqual(['a', 'b']);
  });

  it('drops commands issued at or before the expiry, whatever their state', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true, 0);
    o.enqueue(touch('t', '/g'), false, 5);
    o.transmit(5);
    o.enqueue(set('b', '/h', 1), false, 6);
    const dropped = o.expire(5);
    expect(dropped.map((e) => [e.msg.id, e.state])).toEqual([['a', 'Expired'], ['t', 'Expired']]);
    expect(o.hasSet('/f')).toBe(false);
    expect(ids(o.transmit(7))).toEqual(['b']);
    expect(o.states()).toEqual([{ id: 'b', state: 'Sent' }]);
  });

  it('tracks unanswered sets and commands per control', () => {
    const o = new Outbox();
    o.enqueue(set('a', '/f', 1), true);
    o.enqueue(release('r', '/g'), false);
    expect(o.hasSet('/f')).toBe(true);
    expect(o.has('release', '/g')).toBe(true);
    expect(o.has('touch', '/g')).toBe(false);
  });
});
