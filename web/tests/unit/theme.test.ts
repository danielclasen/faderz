// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { initTheme, setTheme, theme } from '../../src/lib/theme.svelte';

describe('theme of this screen', () => {
  beforeEach(() => localStorage.clear());

  it('starts dark, or as stored, and says so to every reader', () => {
    expect(initTheme()).toBe('dark');
    expect(theme.value).toBe('dark');
    localStorage.setItem('faderz.theme', 'light');
    expect(initTheme()).toBe('light');
    expect(theme.value).toBe('light');
    expect(document.documentElement.dataset['theme']).toBe('light');
  });

  it('switches the page and every reader at once, and keeps the choice', () => {
    initTheme();
    setTheme('light');
    expect(theme.value).toBe('light');
    expect(document.documentElement.dataset['theme']).toBe('light');
    expect(localStorage.getItem('faderz.theme')).toBe('light');
    setTheme('dark');
    expect(theme.value).toBe('dark');
  });
});
