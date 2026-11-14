// Session identity of this tab. The session id survives reconnects and
// replica switches and, through sessionStorage, a reload of the same tab.
// Any other navigation, including a duplicated tab that inherits a copy of
// sessionStorage, starts a new session.

import { randomSessionId, SESSION_ID } from '../net/ids';

const KEY = 'faderz.session';

export interface SessionStorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function sessionId(
  storage: SessionStorageLike | null,
  navigationType: string | undefined,
  generate: () => string = randomSessionId,
): string {
  try {
    const stored = storage?.getItem(KEY) ?? null;
    if (stored && navigationType === 'reload' && SESSION_ID.test(stored)) return stored;
  } catch {
    // Storage blocked: a new session for this load.
  }
  const id = generate();
  try {
    storage?.setItem(KEY, id);
  } catch {
    // Storage blocked: the session lasts for this load only.
  }
  return id;
}

/** Session id of the current page load. */
export function pageSession(): string {
  let storage: SessionStorageLike | null = null;
  try {
    storage = sessionStorage;
  } catch {
    storage = null;
  }
  const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined;
  return sessionId(storage, nav?.type);
}

/** Short display tag of a session id. */
export function sessionTag(session: string): string {
  return session.replace(/[-_]/g, '').slice(0, 6);
}

/** A WebSocket endpoint with the session as its ?session= query parameter. */
export function withSession(endpoint: string, session: string): string {
  const url = new URL(endpoint);
  url.searchParams.set('session', session);
  return url.toString();
}
