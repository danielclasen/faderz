import { describe, expect, it } from 'vitest';
import { getAt, parsePointer, pointer, setAt } from '../../src/lib/state/pointer';
import { CommandLedger } from '../../src/lib/net/ledger';

describe('JSON pointer', () => {
  it('escapes and parses tokens per RFC 6901', () => {
    expect(pointer('tally', 'CAM/1~a')).toBe('/tally/CAM~11~0a');
    expect(parsePointer('/tally/CAM~11~0a')).toEqual(['tally', 'CAM/1~a']);
  });

  it('reads and writes leaves, creating new map entries', () => {
    const root: Record<string, unknown> = { channels: { ch1: { fader: 0 } }, tally: {} };
    setAt(root, '/channels/ch1/fader', -5);
    setAt(root, '/tally/CAM5', 'program');
    expect(getAt(root, '/channels/ch1/fader')).toBe(-5);
    expect(getAt(root, '/tally/CAM5')).toBe('program');
    expect(getAt(root, '/channels/ch9/fader')).toBeUndefined();
  });

  it('copies written values so later mutation of the message does not leak', () => {
    const root: Record<string, unknown> = { vcas: { v: { members: [] } } };
    const members = ['ch1'];
    setAt(root, '/vcas/v/members', members);
    members.push('ch2');
    expect(getAt(root, '/vcas/v/members')).toEqual(['ch1']);
  });
});

describe('command ledger', () => {
  const set = (id: string, path: string, value: number) => ({ type: 'set' as const, id, seq: 1, path, value });

  it('keeps unanswered commands in issue order until resolved', () => {
    const l = new CommandLedger();
    l.add(set('a', '/x', 1));
    l.add({ type: 'touch', id: 'b', path: '/y' });
    l.markSent('a');
    l.markSent('b');
    expect(l.pending().map((m) => m.id)).toEqual(['a', 'b']);
    expect(l.resolve('a')?.id).toBe('a');
    expect(l.pending().map((m) => m.id)).toEqual(['b']);
    expect(l.resolve('zz')).toBeUndefined();
  });

  it('supersedes an unsent set for the same control but never a sent one', () => {
    const l = new CommandLedger();
    l.add(set('a', '/x', 1));
    l.markSent('a');
    l.add(set('b', '/x', 2));
    l.add(set('c', '/x', 3));
    l.add(set('d', '/y', 4));
    expect(l.pending().map((m) => m.id)).toEqual(['a', 'c', 'd']);
  });
});
