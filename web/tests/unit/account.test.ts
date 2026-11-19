import { describe, expect, it } from 'vitest';
import { initials, isEmail } from '../../src/lib/account';

describe('initials', () => {
  it('takes the first and last word of an email address local part, not its domain', () => {
    expect(initials('jane.doe@example.org')).toBe('JD');
    expect(initials('jane.m.doe@example.org')).toBe('JD');
    expect(initials('j_doe+ops@example.org')).toBe('JO');
  });
  it('takes one letter of a one-word name and two of a longer one', () => {
    expect(initials('alice')).toBe('A');
    expect(initials('Alice Anders')).toBe('AA');
    expect(initials('token')).toBe('T');
  });
  it('keeps a letter outside the basic plane whole', () => {
    expect(initials('\u{1D49C}lex Bo')).toBe('\u{1D49C}B');
  });
  it('gives ? where no word has a letter or digit', () => {
    expect(initials('@example.org')).toBe('?');
    expect(initials('--')).toBe('?');
  });
});

describe('isEmail', () => {
  it('tells an address from a user name', () => {
    expect(isEmail('jane.doe@example.org')).toBe(true);
    expect(isEmail('alice')).toBe(false);
  });
});
