<script lang="ts">
  // The sign-in prompt over the console while it is not signed in: the
  // API token in token mode, a link to the identity provider in oidc mode.
  // When the link leaves Live, which it does when the server closes the
  // WebSocket of an ended session, and every few seconds while it is down,
  // the session is read again; an ended one goes to the sign-in in oidc
  // mode (AuthStore.signInIfSignedOut) or shows the prompt.
  import { getAuth } from '../lib/auth.svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { signInUrl } from '../lib/net/auth';

  const RECHECK_MS = 5000;
  const auth = getAuth();
  const mixer = getMixer();
  let token = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  $effect(() => {
    mixer.viewer = auth.session?.role === 'viewer';
  });
  // Derived, so that only a change of either re-runs the effect below, not
  // every session read or status update.
  const mode = $derived(auth.session?.mode);
  const down = $derived(mixer.status.link !== 'Live');
  $effect(() => {
    if (!down || (mode !== 'token' && mode !== 'oidc')) return;
    void auth.recheck();
    const t = setInterval(() => void auth.recheck(), RECHECK_MS);
    return () => clearInterval(t);
  });

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      if (await auth.signIn(token)) {
        token = '';
      } else {
        error = 'Wrong token.';
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

{#if auth.signedOut}
  <div class="backdrop" role="dialog" aria-modal="true" aria-labelledby="signin-title">
    <form class="signin" data-testid="signin" onsubmit={submit}>
      <h2 id="signin-title">Sign in to faderz</h2>
      {#if auth.session?.mode === 'oidc'}
        <p>Not signed in, or the session has ended.</p>
        <a class="key go" href={signInUrl(auth.base, location)}>SIGN IN</a>
      {:else}
        <label>
          <span>API token</span>
          <!-- svelte-ignore a11y_autofocus -->
          <input type="password" autocomplete="current-password" bind:value={token} autofocus />
        </label>
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        <button class="key go" type="submit" disabled={busy || token.length === 0}>SIGN IN</button>
      {/if}
    </form>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 900;
    display: grid;
    place-items: center;
    padding: 16px;
    background: rgb(0 0 0 / 0.6);
  }
  .signin {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: min(340px, 100%);
    padding: 16px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    color: var(--text);
  }
  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  p {
    margin: 0;
    color: var(--text-dim);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--text-dim);
  }
  input {
    padding: 6px 8px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--well);
    color: var(--text);
    font: inherit;
  }
  input:focus-visible {
    outline: 2px solid var(--focus);
  }
  .error {
    color: var(--mute);
  }
  .go {
    align-self: flex-end;
    padding: 6px 12px;
    text-decoration: none;
  }
</style>
