import { describe, expect, it } from 'vitest';
import { compareVersions, newVersion } from '../../src/lib/newVersion';

const build = (faderz: string) => ({ faderz, samplez: '0.16.0' });

describe('newVersion', () => {
  it('names a newer version a replica serves, until it is dismissed, and the next one again', () => {
    expect(newVersion(build('0.4.6'), null, '0.4.6')).toBeNull();
    expect(newVersion(null, null, '0.4.6')).toBeNull();
    expect(newVersion(build('0.4.7'), null, '0.4.6')).toBe('0.4.7');
    expect(newVersion(build('0.4.7'), '0.4.7', '0.4.6')).toBeNull();
    expect(newVersion(build('0.4.8'), '0.4.7', '0.4.6')).toBe('0.4.8');
  });

  it('says nothing of an older replica, as a page from a rolled one reconnecting to one not rolled yet, or a rollback, sees', () => {
    expect(newVersion(build('0.4.6'), null, '0.4.7')).toBeNull();
    expect(newVersion(build('0.3.9'), null, '0.4.0')).toBeNull();
  });

  it('orders prereleases below their release and by their identifiers', () => {
    expect(compareVersions('0.4.7', '0.4.7-rc.1')).toBeGreaterThan(0);
    expect(compareVersions('0.4.7-rc.2', '0.4.7-rc.10')).toBeLessThan(0);
    expect(compareVersions('0.4.7-alpha', '0.4.7-1')).toBeGreaterThan(0);
    expect(compareVersions('0.10.0', '0.9.9')).toBeGreaterThan(0);
    expect(compareVersions('not-a-version', '0.4.7')).toBeNull();
    expect(newVersion(build('0.4.7-rc.1'), null, '0.4.6')).toBe('0.4.7-rc.1');
    expect(newVersion(build('0.4.7-rc.1'), null, '0.4.7')).toBeNull();
  });
});
