import { beforeEach, describe, expect, it } from 'vitest';
import { MixerStore } from '../../src/lib/state/mixer.svelte';
import type { ClientMessage, DeltaMessage, ReplicaRef, ServerMessage, SnapshotMessage } from '../../src/lib/contract/types';
import { exampleShape, initialState } from '../../dev/mock/shape';

const r1 = { id: 'r1', role: 'leader' as const };
const r2 = { id: 'r2', role: 'follower' as const };
const FADER = '/channels/ch1/fader';

function snapshot(revision: number, replica: ReplicaRef = r1, patch?: (s: SnapshotMessage) => void): SnapshotMessage {
  const shape = exampleShape();
  const s: SnapshotMessage = { type: 'snapshot', clientId: 'me', replica, revision, shape, state: initialState(shape), owners: {} };
  patch?.(s);
  return s;
}

function delta(revision: number, changes: DeltaMessage['changes'], extra: Partial<DeltaMessage> = {}): DeltaMessage {
  return { type: 'delta', replica: r1, revision, ...(changes ? { changes } : {}), ...extra };
}

describe('MixerStore', () => {
  let sent: ClientMessage[];
  let up: boolean;
  let mixer: MixerStore;
  const recv = (m: ServerMessage) => mixer.receive(JSON.stringify(m));

  beforeEach(() => {
    sent = [];
    up = true;
    mixer = new MixerStore('me');
    mixer.attach({
      send: (m) => {
        if (up) sent.push(m);
        return up;
      },
    });
  });

  it('says hello, then goes live on the snapshot', () => {
    mixer.opened('ws://a/api/v1/ws');
    expect(sent).toEqual([{ type: 'hello', clientId: 'me' }]);
    expect(mixer.link).toBe('syncing');
    recv(snapshot(10));
    expect(mixer.link).toBe('live');
    expect(mixer.revision).toBe(10);
    expect(mixer.value(FADER)).toBe(0);
  });

  it('shows a local value while its set is in flight and settles on the echo', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, -6);
    const cmd = sent.at(-1) as Extract<ClientMessage, { type: 'set' }>;
    expect(cmd).toMatchObject({ type: 'set', path: FADER, value: -6 });
    expect(mixer.value(FADER)).toBe(-6);
    // Another client's older value arrives first: the local value stays.
    recv(delta(11, [{ path: FADER, value: -20 }]));
    expect(mixer.value(FADER)).toBe(-6);
    recv(delta(12, [{ path: FADER, value: -6 }], { origin: { clientId: 'me', commandId: cmd.id, seq: cmd.seq } }));
    expect(mixer.overlay.size).toBe(0);
    expect(mixer.value(FADER)).toBe(-6);
    expect(mixer.pendingCount).toBe(0);
  });

  it('asks for a resync on a revision gap and ignores deltas until it arrives', () => {
    mixer.opened('a');
    recv(snapshot(10));
    recv(delta(12, [{ path: FADER, value: -1 }]));
    expect(sent.at(-1)).toEqual({ type: 'resync', lastRevision: 10 });
    expect(mixer.link).toBe('syncing');
    recv(delta(13, [{ path: FADER, value: -2 }]));
    expect(mixer.value(FADER)).toBe(0);
    recv({ type: 'resume', clientId: 'me', replica: r1, revision: 12, deltas: [delta(11, [{ path: FADER, value: -3 }]), delta(12, [{ path: FADER, value: -1 }])] });
    expect(mixer.revision).toBe(12);
    expect(mixer.value(FADER)).toBe(-1);
    expect(mixer.link).toBe('live');
  });

  it('reverts to the current value and reports the owner on 409', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, 5);
    const cmd = sent.at(-1) as Extract<ClientMessage, { type: 'set' }>;
    recv({ type: 'error', replica: r1, code: 409, commandId: cmd.id, path: FADER, current: 0, owner: 'mcu-1', message: 'owned' });
    expect(mixer.value(FADER)).toBe(0);
    expect(mixer.conflicts.has(FADER)).toBe(true);
    expect(mixer.notices.at(-1)).toMatchObject({ kind: 'conflict' });
    expect(mixer.notices.at(-1)!.text).toContain('mcu-1');
  });

  it('fails over: resumes from its revision, resends unanswered commands with the same ids, keeps local values', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, -12);
    const lost = sent.at(-1) as Extract<ClientMessage, { type: 'set' }>;
    mixer.closed();
    up = false;
    mixer.set('/channels/ch2/mute', true);
    expect(mixer.link).toBe('connecting');
    up = true;
    sent = [];
    mixer.opened('b');
    expect(sent[0]).toEqual({ type: 'hello', clientId: 'me', lastRevision: 10 });
    recv({ type: 'resume', clientId: 'me', replica: r2, revision: 11, deltas: [delta(11, [{ path: '/channels/ch5/mute', value: true }])] });
    expect(mixer.failover).toMatchObject({ from: 'r1', to: 'r2' });
    const resent = sent.slice(1);
    expect(resent.map((m) => (m as { id?: string }).id)[0]).toBe(lost.id);
    expect(resent).toHaveLength(2);
    expect(mixer.value(FADER)).toBe(-12);
  });

  it('settles a command the new replica already applied via its resume delta', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, -3);
    const cmd = sent.at(-1) as Extract<ClientMessage, { type: 'set' }>;
    mixer.closed();
    sent = [];
    mixer.opened('b');
    recv({
      type: 'resume', clientId: 'me', replica: r2, revision: 11,
      deltas: [delta(11, [{ path: FADER, value: -3 }], { origin: { clientId: 'me', commandId: cmd.id, seq: cmd.seq } })],
    });
    expect(sent.slice(1)).toHaveLength(0);
    expect(mixer.pendingCount).toBe(0);
  });

  it('keeps an in-flight value over a snapshot after failover so the fader does not jump', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, -8);
    mixer.closed();
    mixer.opened('b');
    recv(snapshot(40, r2));
    expect(mixer.value(FADER)).toBe(-8);
  });

  it('takes a held control back after resync when its ownership lapsed', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.touch(FADER);
    const touch = sent.at(-1) as Extract<ClientMessage, { type: 'touch' }>;
    recv(delta(11, undefined, { owners: [{ path: FADER, owner: 'me' }], origin: { clientId: 'me', commandId: touch.id } }));
    expect(mixer.owners[FADER]).toBe('me');
    mixer.closed();
    sent = [];
    mixer.opened('b');
    recv(snapshot(20, r2));
    expect(sent.at(-1)).toMatchObject({ type: 'touch', path: FADER });
    expect((sent.at(-1) as { id: string }).id).not.toBe(touch.id);
  });

  it('does not take back a control it still owns after a resume', () => {
    mixer.opened('a');
    recv(snapshot(10, r1, (s) => (s.owners[FADER] = 'me')));
    mixer.touch(FADER);
    expect(sent.filter((m) => m.type === 'touch')).toHaveLength(0);
    mixer.closed();
    sent = [];
    mixer.opened('b');
    recv({ type: 'resume', clientId: 'me', replica: r2, revision: 10, deltas: [] });
    expect(sent.filter((m) => m.type === 'touch')).toHaveLength(0);
  });

  it('drops a stale ack without disturbing a newer in-flight value', () => {
    mixer.opened('a');
    recv(snapshot(10));
    mixer.set(FADER, -1);
    const first = sent.at(-1) as Extract<ClientMessage, { type: 'set' }>;
    mixer.set(FADER, -2);
    recv({ type: 'ack', replica: r1, commandId: first.id, result: 'stale', revision: 10 });
    expect(mixer.value(FADER)).toBe(-2);
    expect(mixer.pendingCount).toBe(1);
  });

  it('records meter frames with the replica that measured them', () => {
    mixer.opened('a');
    recv(snapshot(10));
    recv({ type: 'meters', replica: r1, channels: { ch1: { peak: [-10], rms: [-20] } }, buses: {}, auxes: {}, mixMinus: {} });
    expect(mixer.meterReplica).toBe('r1');
    expect(mixer.meters.get('channels', 'ch1')).toEqual({ peak: [-10], rms: [-20] });
  });
});
