// The console's sign-in state, shared by the sign-in prompt and the status
// bar through context.

import { getContext } from 'svelte';
import { fetchSession, needsSignIn, signInWithToken, signOut, type Session } from './net/auth';

const KEY = Symbol('auth');

export class AuthStore {
  /** null until known, and where the server has no session route. */
  session = $state<Session | null>(null);
  /** Not signed in, in a mode that signs in: the prompt shows. */
  readonly signedOut = $derived(needsSignIn(this.session));

  constructor(
    /** The REST base the session, login and logout routes are under. */
    readonly base: string,
    private readonly fetchFn: typeof fetch = (...a) => fetch(...a),
  ) {}

  async refresh(): Promise<void> {
    this.session = await fetchSession(this.base, this.fetchFn);
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
