// The console's sign-in state, shared by the sign-in prompt and the status
// bar through context.

import { getContext } from 'svelte';
import { fetchSession, mayRedirect, needsSignIn, signInUrl, signInWithToken, signOut, type Session } from './net/auth';

const KEY = Symbol('auth');

/** Where the store sends the browser to sign in, and what it remembers between page loads. */
export interface Browser {
  location(): { pathname: string; search: string };
  assign(url: string): void;
  /** sessionStorage, or null where it is blocked. */
  storage(): Pick<Storage, 'getItem' | 'setItem'> | null;
}

const browser: Browser = {
  location: () => location,
  assign: (url) => location.assign(url),
  storage: () => {
    try {
      return window.sessionStorage;
    } catch {
      return null;
    }
  },
};

export class AuthStore {
  /** null until known, and where the server has no session route. */
  session = $state<Session | null>(null);
  /** Not signed in, in a mode that signs in: the prompt shows. */
  readonly signedOut = $derived(needsSignIn(this.session));

  constructor(
    /** The REST base the session, login and logout routes are under. */
    readonly base: string,
    private readonly fetchFn: typeof fetch = (...a) => fetch(...a),
    private readonly browser_: Browser = browser,
  ) {}

  async refresh(): Promise<void> {
    this.session = await fetchSession(this.base, this.fetchFn);
  }

  /**
   * In oidc mode, not signed in: sends the browser to the sign-in, which
   * returns to this page. Not when it did so within the last 30 s, so a
   * sign-in that does not take ends at the prompt rather than in a loop.
   * True when it went.
   */
  signInIfSignedOut(now = Date.now()): boolean {
    if (!this.signedOut || this.session?.mode !== 'oidc') return false;
    if (!mayRedirect(this.browser_.storage(), now)) return false;
    this.browser_.assign(signInUrl(this.base, this.browser_.location()));
    return true;
  }

  /** Reads the session again and signs in when it has ended. */
  async recheck(): Promise<void> {
    await this.refresh();
    this.signInIfSignedOut();
  }

  /** Token mode: true once signed in, false for a wrong token. */
  async signIn(token: string): Promise<boolean> {
    if (!(await signInWithToken(this.base, token, this.fetchFn))) return false;
    await this.refresh();
    return true;
  }

  /** Clears the session cookie; returns where the browser goes next, if anywhere. */
  async signOut(): Promise<string | null> {
    const next = await signOut(this.base, this.fetchFn);
    if (!next) await this.refresh();
    return next;
  }
}

export function authContext(store: AuthStore): Map<symbol, AuthStore> {
  return new Map([[KEY, store]]);
}

export function getAuth(): AuthStore {
  return getContext<AuthStore>(KEY);
}
