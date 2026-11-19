// Theme of this screen, dark or light: a per-screen preference kept in
// localStorage, set from the account menu or the Setup view.

export type Theme = 'dark' | 'light';
const KEY = 'faderz.theme';

export const theme = $state<{ value: Theme }>({ value: 'dark' });

function stored(): Theme | null {
  try {
    const v = localStorage.getItem(KEY);
    return v === 'dark' || v === 'light' ? v : null;
  } catch {
    return null;
  }
}

export function initTheme(): Theme {
  const t = stored() ?? 'dark';
  theme.value = t;
  document.documentElement.dataset['theme'] = t;
  return t;
}

export function setTheme(t: Theme): void {
  theme.value = t;
  document.documentElement.dataset['theme'] = t;
  try {
    localStorage.setItem(KEY, t);
  } catch {
    // Storage blocked: the choice lasts for this page only.
  }
}
