import { describe, expect, it } from 'vitest';
import { getAt, parsePointer, pointer, setAt } from '../../src/lib/state/pointer';

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
