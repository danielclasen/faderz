// Who the console is signed in as, and the sign-in of the auth modes that
// have one: in oidc mode the browser goes to the server's sign-in route,
// which sends it on to the identity provider; in token mode the operator
// enters the API token, which the server exchanges for a session cookie.

export type AuthMode = 'none' | 'token' | 'proxy' | 'oidc';
export type Role = 'operator' | 'viewer';

/** GET /session (api/openapi.yaml Session). */
export interface Session {
  mode: AuthMode;
  user: string | null;
  role: Role | null;
}

const WRITE = { 'Content-Type': 'application/json', 'X-Samplez-Request': '1' };
const MODES: readonly string[] = ['none', 'token', 'proxy', 'oidc'];

export function isSession(v: unknown): v is Session {
  if (typeof v !== 'object' || v === null) return false;
  const s = v as Record<string, unknown>;
  return (
    typeof s['mode'] === 'string' &&
    MODES.includes(s['mode']) &&
    (s['user'] === null || typeof s['user'] === 'string') &&
    (s['role'] === null || s['role'] === 'operator' || s['role'] === 'viewer')
  );
}

/**
 * The session the REST base `base` reports; null when it answers
 * anything but a session, as a server without the route or the error page
 * of a proxy, or cannot be reached.
 */
export async function fetchSession(base: string, fetchFn: typeof fetch = fetch): Promise<Session | null> {
  try {
    const res = await fetchFn(`${base}/session`, { cache: 'no-store' });
    if (!res.ok) return null;
    const v = (await res.json()) as unknown;
    return isSession(v) ? v : null;
  } catch {
    return null;
  }
}

/** Not signed in, in a mode the console signs in to. */
export function needsSignIn(s: Session | null): boolean {
  return s !== null && s.role === null && (s.mode === 'oidc' || s.mode === 'token');
}

/** The oidc mode sign-in route, returning to the page's path and query. */
export function signInUrl(base: string, location: { pathname: string; search: string }): string {
  return `${new URL(base).pathname}/login?return=${encodeURIComponent(location.pathname + location.search)}`;
}

/** Exchanges the API token for a session cookie: true when it is set, false for a wrong token. */
export async function signInWithToken(base: string, token: string, fetchFn: typeof fetch = fetch): Promise<boolean> {
  const res = await fetchFn(`${base}/login`, { method: 'POST', headers: WRITE, body: JSON.stringify({ token }) });
  if (res.status === 204) return true;
  if (res.status === 401) return false;
  throw new Error(`sign-in failed: HTTP ${res.status}`);
}

/** Clears the session cookie; returns where the browser goes next to sign out at the identity provider, if anywhere. */
export async function signOut(base: string, fetchFn: typeof fetch = fetch): Promise<string | null> {
  const res = await fetchFn(`${base}/logout`, { method: 'POST', headers: WRITE });
  if (!res.ok) throw new Error(`sign-out failed: HTTP ${res.status}`);
  const body = (await res.json()) as { redirect?: unknown };
  return typeof body.redirect === 'string' ? body.redirect : null;
}

/**
 * Whether the page may send the browser to the oidc sign-in on its own:
 * not when it did within `windowMs`, so a sign-in that does not take, as a
 * Secure cookie on a plain HTTP page, ends at a prompt rather than in a
 * redirect loop. `store` is sessionStorage, or null where it is blocked.
 */
export function mayRedirect(store: Pick<Storage, 'getItem' | 'setItem'> | null, now: number, windowMs = 30_000): boolean {
  const KEY = 'faderz.signInAt';
  if (!store) return true;
  try {
    const last = store.getItem(KEY);
    if (last !== null && now - Number(last) < windowMs) return false;
    store.setItem(KEY, String(now));
  } catch {
    // Without storage every visit may redirect once.
  }
  return true;
}
