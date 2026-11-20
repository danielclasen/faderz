import { describe, expect, it } from 'vitest';
import { newVersion } from '../../src/lib/newVersion';

const build = (faderz: string) => ({ faderz, samplez: '0.16.0' });

describe('newVersion', () => {
  it('names another version a replica serves, until it is dismissed, and the next one again', () => {
    expect(newVersion(build('0.4.6'), null, '0.4.6')).toBeNull();
    expect(newVersion(null, null, '0.4.6')).toBeNull();
    expect(newVersion(build('0.4.7'), null, '0.4.6')).toBe('0.4.7');
    expect(newVersion(build('0.4.7'), '0.4.7', '0.4.6')).toBeNull();
    expect(newVersion(build('0.4.8'), '0.4.7', '0.4.6')).toBe('0.4.8');
  });
});
