export type Theme = 'dark' | 'light';
const KEY = 'faderz.theme';

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
  document.documentElement.dataset['theme'] = t;
  return t;
}

export function setTheme(t: Theme): void {
  document.documentElement.dataset['theme'] = t;
  try {
    localStorage.setItem(KEY, t);
  } catch {
    // Storage blocked: the choice lasts for this page only.
  }
}
