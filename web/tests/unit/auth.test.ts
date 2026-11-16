// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { fetchSession, isSession, mayRedirect, needsSignIn, signInUrl, signInWithToken, signOut, type Session } from '../../src/lib/net/auth';
import { AuthStore, authContext } from '../../src/lib/auth.svelte';
import { MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import SignIn from '../../src/components/SignIn.svelte';
import StatusBar from '../../src/components/StatusBar.svelte';

const BASE = 'http://mixer.example/api/v1';

/** A fetch answering each `METHOD path` from `routes`, recording every request. */
function fakeFetch(routes: Record<string, () => Response>) {
  const calls: { method: string; path: string; headers: Record<string, string>; body: string | null }[] = [];
  const f = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    const method = init?.method ?? 'GET';
    calls.push({ method, path: url.pathname, headers: (init?.headers ?? {}) as Record<string, string>, body: (init?.body as string) ?? null });
    const route = routes[`${method} ${url.pathname}`];
    return route ? route() : new Response('not found', { status: 404 });
  }) as typeof fetch;
  return { f, calls };
}

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { 'Content-Type': 'application/json' } });
const session = (s: Session) => () => json(s);

describe('the session route', () => {
  it('reads a session, and nothing from a server without the route', async () => {
    const s: Session = { mode: 'oidc', user: 'alice', role: 'viewer' };
    expect(await fetchSession(BASE, fakeFetch({ 'GET /api/v1/session': session(s) }).f)).toEqual(s);
    expect(await fetchSession(BASE, fakeFetch({}).f)).toBeNull();
    const html = () => new Response('<!doctype html>', { status: 200 });
    expect(await fetchSession(BASE, fakeFetch({ 'GET /api/v1/session': html }).f)).toBeNull();
    const down = (async () => {
      throw new TypeError('network');
    }) as typeof fetch;
    expect(await fetchSession(BASE, down)).toBeNull();
    expect(isSession({ mode: 'oidc', user: null, role: 'admin' })).toBe(false);
    expect(isSession({ mode: 'ldap', user: null, role: null })).toBe(false);
  });

  it('wants a sign-in in token and oidc mode without a role only', () => {
    expect(needsSignIn({ mode: 'oidc', user: null, role: null })).toBe(true);
    expect(needsSignIn({ mode: 'token', user: null, role: null })).toBe(true);
    expect(needsSignIn({ mode: 'oidc', user: 'a', role: 'viewer' })).toBe(false);
    expect(needsSignIn({ mode: 'proxy', user: null, role: null })).toBe(false);
    expect(needsSignIn(null)).toBe(false);
  });
});

describe('signing in and out', () => {
  it('returns to the page after the oidc sign-in', () => {
    expect(signInUrl(BASE, { pathname: '/', search: '?endpoints=/api/v1/ws' })).toBe('/api/v1/login?return=%2F%3Fendpoints%3D%2Fapi%2Fv1%2Fws');
  });

  it('exchanges the token with the request header the server wants', async () => {
    const { f, calls } = fakeFetch({ 'POST /api/v1/login': () => new Response(null, { status: 204 }) });
    expect(await signInWithToken(BASE, 't0k', f)).toBe(true);
    expect(calls[0]!.headers['X-Samplez-Request']).toBe('1');
    expect(JSON.parse(calls[0]!.body!)).toEqual({ token: 't0k' });
    const wrong = fakeFetch({ 'POST /api/v1/login': () => new Response(null, { status: 401 }) });
    expect(await signInWithToken(BASE, 'x', wrong.f)).toBe(false);
    await expect(signInWithToken(BASE, 'x', fakeFetch({}).f)).rejects.toThrow('404');
  });

  it('signs out, with the identity provider when it has an end-session URL', async () => {
    const at = 'https://idp.example/logout?client_id=faderz';
    expect(await signOut(BASE, fakeFetch({ 'POST /api/v1/logout': () => json({ redirect: at }) }).f)).toBe(at);
    expect(await signOut(BASE, fakeFetch({ 'POST /api/v1/logout': () => json({ redirect: null }) }).f)).toBeNull();
  });

  it('redirects on its own once per window, so a sign-in that does not take cannot loop', () => {
    const m = new Map<string, string>();
    const store = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
    expect(mayRedirect(store, 100_000)).toBe(true);
    expect(mayRedirect(store, 110_000)).toBe(false);
    expect(mayRedirect(store, 131_000)).toBe(true);
    expect(mayRedirect(null, 0)).toBe(true);
  });
});

function mountWith(component: typeof SignIn | typeof StatusBar, auth: AuthStore, mixer = new MixerStore()) {
  mixer.attach({ post: () => {} });
  const target = document.createElement('div');
  document.body.append(target);
  const c = mount(component, { target, context: new Map<symbol, unknown>([...mixerContext(mixer), ...authContext(auth)]) });
  flushSync();
  return { target, mixer, done: () => unmount(c) };
}

const settle = async () => {
  for (let i = 0; i < 5; i++) await tick();
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
};

describe('the sign-in prompt', () => {
  it('takes the token in token mode and closes once signed in', async () => {
    let signedIn = false;
    const { f, calls } = fakeFetch({
      'GET /api/v1/session': () => json({ mode: 'token', user: null, role: signedIn ? 'operator' : null }),
      'POST /api/v1/login': () => {
        const ok = JSON.parse(calls.at(-1)!.body!).token === 'right';
        signedIn ||= ok;
        return new Response(null, { status: ok ? 204 : 401 });
      },
    });
    const auth = new AuthStore(BASE, f);
    await auth.refresh();
    const { target, done } = mountWith(SignIn, auth);
    const form = () => target.querySelector<HTMLFormElement>('[data-testid="signin"]');
    expect(form()).not.toBeNull();
    const input = target.querySelector('input')!;
    const submit = async (value: string) => {
      input.value = value;
      input.dispatchEvent(new Event('input'));
      flushSync();
      form()!.dispatchEvent(new Event('submit', { cancelable: true }));
      await settle();
    };
    await submit('wrong');
    expect(target.querySelector('[role="alert"]')?.textContent).toBe('Wrong token.');
    await submit('right');
    expect(form()).toBeNull();
    done();
  });

  it('links to the identity provider in oidc mode', async () => {
    const { f } = fakeFetch({ 'GET /api/v1/session': session({ mode: 'oidc', user: null, role: null }) });
    const auth = new AuthStore(BASE, f);
    await auth.refresh();
    const { target, done } = mountWith(SignIn, auth);
    const link = target.querySelector<HTMLAnchorElement>('[data-testid="signin"] a')!;
    expect(new URL(link.href).pathname).toBe('/api/v1/login');
    expect(target.querySelector('input')).toBeNull();
    done();
  });

  it('locks the write controls of a viewer', async () => {
    const { f } = fakeFetch({ 'GET /api/v1/session': session({ mode: 'oidc', user: 'vera', role: 'viewer' }) });
    const auth = new AuthStore(BASE, f);
    await auth.refresh();
    const { target, mixer, done } = mountWith(SignIn, auth);
    flushSync();
    expect(target.querySelector('[data-testid="signin"]')).toBeNull();
    expect(mixer.lockReason).toBe('signed in as a viewer');
    expect(mixer.locked).toBe(true);
    done();
  });
});

describe('the signed-in user in the status bar', () => {
  it('shows who and offers to sign out', async () => {
    const { f, calls } = fakeFetch({
      'GET /api/v1/session': session({ mode: 'oidc', user: 'alice', role: 'operator' }),
      'POST /api/v1/logout': () => json({ redirect: null }),
    });
    const auth = new AuthStore(BASE, f);
    await auth.refresh();
    const { target, done } = mountWith(StatusBar, auth);
    expect(target.querySelector('[data-testid="user"]')?.textContent).toBe('alice (operator)');
    const out = [...target.querySelectorAll('button')].find((b) => b.textContent === 'Sign out')!;
    out.click();
    await settle();
    expect(calls.some((c) => c.method === 'POST' && c.path === '/api/v1/logout')).toBe(true);
    done();
  });

  it('shows nobody in none mode', async () => {
    const { f } = fakeFetch({ 'GET /api/v1/session': session({ mode: 'none', user: null, role: 'operator' }) });
    const auth = new AuthStore(BASE, f);
    await auth.refresh();
    const { target, done } = mountWith(StatusBar, auth);
    expect(target.querySelector('[data-testid="user"]')).toBeNull();
    done();
  });
});

/** A browser that records where the store sends it. */
function fakeBrowser() {
  const store = new Map<string, string>();
  const went: string[] = [];
  return {
    went,
    browser: {
      location: () => ({ pathname: '/', search: '?endpoints=/api/v1/ws' }),
      assign: (url: string) => void went.push(url),
      storage: () => ({ getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) }),
    },
  };
}

describe('a session that ends while the console runs', () => {
  it('reads the session again on a 401 from the REST client', async () => {
    const { RestClient } = await import('../../src/lib/net/rest');
    let unauthorized = 0;
    const rest = new RestClient(() => [BASE], 1, 0, 1000, () => unauthorized++);
    const original = globalThis.fetch;
    globalThis.fetch = (async () => json({ code: 401, message: 'not authenticated' }, 401)) as typeof fetch;
    try {
      await expect(rest.request('GET', '/state')).rejects.toThrow();
    } finally {
      globalThis.fetch = original;
    }
    expect(unauthorized).toBe(1);
  });

  it('goes to the sign-in in oidc mode, once per window', async () => {
    let signedIn = true;
    const { f } = fakeFetch({ 'GET /api/v1/session': () => json({ mode: 'oidc', user: signedIn ? 'alice' : null, role: signedIn ? 'operator' : null }) });
    const { browser, went } = fakeBrowser();
    const auth = new AuthStore(BASE, f, browser);
    await auth.recheck();
    expect(went).toEqual([]);
    signedIn = false;
    await auth.recheck();
    expect(went).toEqual(['/api/v1/login?return=%2F%3Fendpoints%3D%2Fapi%2Fv1%2Fws']);
    await auth.recheck();
    expect(went).toHaveLength(1);
    expect(auth.signedOut).toBe(true);
  });

  it('reads the session again when the link leaves Live and signs in', async () => {
    let signedIn = true;
    const { f, calls } = fakeFetch({ 'GET /api/v1/session': () => json({ mode: 'oidc', user: signedIn ? 'alice' : null, role: signedIn ? 'operator' : null }) });
    const { browser, went } = fakeBrowser();
    const auth = new AuthStore(BASE, f, browser);
    await auth.refresh();
    const mixer = new MixerStore();
    mixer.status = { ...mixer.status, link: 'Live' };
    const { done } = mountWith(SignIn, auth, mixer);
    await settle();
    const reads = calls.length;
    expect(went).toEqual([]);
    // The server closes the WebSocket of the ended session.
    signedIn = false;
    mixer.status = { ...mixer.status, link: 'Connecting' };
    await settle();
    expect(calls.length).toBeGreaterThan(reads);
    expect(went).toHaveLength(1);
    done();
  });
});
