<script lang="ts">
  // The signed-in user at the header's right end: a pill with the user's
  // initials that opens a menu. The menu names the role, and the user
  // only where that is no email address; its entries are a list of
  // settings, then Sign out.
  import { tick } from 'svelte';
  import { getAuth } from '../lib/auth.svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { follow, type Placement } from '../lib/anchor';
  import { initials, isEmail } from '../lib/account';
  import { setTheme, theme } from '../lib/theme.svelte';

  const mixer = getMixer();
  // Absent where the header is mounted without the sign-in state.
  const auth = getAuth() as ReturnType<typeof getAuth> | undefined;
  const session = $derived(auth?.session ?? null);
  /** Who is signed in, where a mode names anybody or has a role to show. */
  const user = $derived(session?.role && session.mode !== 'none' ? (session.user ?? 'token') : null);
  /** The user's name where it may be shown: never an email address. */
  const name = $derived(user !== null && !isEmail(user) ? user : null);
  const label = $derived(`Account${name ? ` ${name}` : ''}, ${session?.role ?? ''}`);

  interface Entry {
    id: string;
    label: string;
    run: () => void | Promise<void>;
  }
  const entries = $derived<Entry[]>([
    {
      id: 'theme',
      label: `Switch to ${theme.value === 'dark' ? 'light' : 'dark'} theme`,
      run: () => setTheme(theme.value === 'dark' ? 'light' : 'dark'),
    },
    ...(session?.role != null && (session.mode === 'token' || session.mode === 'oidc') ? [{ id: 'sign-out', label: 'Sign out', run: leave }] : []),
  ]);

  async function leave(): Promise<void> {
    if (!auth) return;
    try {
      const next = await auth.signOut();
      if (next) location.assign(next);
    } catch (e) {
      mixer.notify('error', e instanceof Error ? e.message : String(e));
    }
  }

  let open = $state(false);
  let pill = $state<HTMLButtonElement>();
  let menu = $state<HTMLDivElement>();
  let place = $state<Placement>({ left: 0, top: 0 });
  // Under the pill, its right edge on the pill's, moved in where that
  // runs off the viewport.
  $effect(() => {
    if (!open || !pill || !menu) return;
    return follow(pill, menu, 'below', 'end', (p) => (place = p));
  });

  const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
  async function show(focus: 'first' | 'last' | null): Promise<void> {
    open = true;
    await tick();
    const all = items();
    (focus === 'first' ? all[0] : focus === 'last' ? all.at(-1) : null)?.focus();
  }
  function hide(refocus: boolean): void {
    open = false;
    if (refocus) pill?.focus();
  }
  function pillKey(e: KeyboardEvent): void {
    if (e.key === 'Escape' && open) {
      e.preventDefault();
      hide(true);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      void show(e.key === 'ArrowDown' ? 'first' : 'last');
    }
  }
  function menuKey(e: KeyboardEvent): void {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    const move = (to: number) => {
      e.preventDefault();
      all[(to + all.length) % all.length]?.focus();
    };
    if (e.key === 'ArrowDown') move(at + 1);
    else if (e.key === 'ArrowUp') move(at - 1);
    else if (e.key === 'Home') move(0);
    else if (e.key === 'End') move(all.length - 1);
    else if (e.key === 'Escape') {
      e.preventDefault();
      hide(true);
    } else if (e.key === 'Tab') hide(false);
  }
  /** A press anywhere but on the pill or the menu closes it. */
  function outside(e: PointerEvent): void {
    if (!open) return;
    const t = e.target as Node;
    if (!pill?.contains(t) && !menu?.contains(t)) hide(false);
  }
  async function choose(entry: Entry): Promise<void> {
    hide(true);
    await entry.run();
  }
</script>

<svelte:window onpointerdown={outside} />

{#if user !== null}
  <button
    bind:this={pill}
    class="pill"
    class:open
    aria-haspopup="menu"
    aria-expanded={open}
    aria-controls="account-menu"
    aria-label={label}
    title={label}
    data-testid="account"
    onclick={() => (open ? hide(false) : void show(null))}
    onkeydown={pillKey}><span class="fixed">{initials(user)}</span></button
  >
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div
      bind:this={menu}
      class="menu"
      id="account-menu"
      role="menu"
      aria-label="Account"
      tabindex="-1"
      style:left="{place.left}px"
      style:top="{place.top ?? 0}px"
      onkeydown={menuKey}
    >
      <div class="who" role="presentation">
        <span class="badge fixed" aria-hidden="true">{initials(user)}</span>
        <span class="names">
          <b>{name ?? 'Signed in'}</b>
          <span data-testid="account-role">{session?.role}</span>
        </span>
      </div>
      {#each entries as entry (entry.id)}
        <button class="item" role="menuitem" tabindex="-1" data-entry={entry.id} onclick={() => choose(entry)}>{entry.label}</button>
      {/each}
    </div>
  {/if}
{/if}

<style>
  .pill {
    flex: none;
    box-sizing: border-box;
    min-width: 28px;
    height: 26px;
    padding: 0 6px;
    border: 1px solid var(--key-edge);
    border-radius: 13px;
    background: var(--panel-2);
    color: var(--text);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }
  .pill:hover,
  .pill.open {
    border-color: var(--text-dim);
  }
  .pill:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 1px;
  }
  /* Opens under the header over the view, placed by follow(); it moves
     nothing. */
  .menu {
    position: fixed;
    z-index: 40;
    box-sizing: border-box;
    min-width: 200px;
    max-width: calc(100vw - 16px);
    padding: 4px;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
    font-size: 12px;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 6px 8px;
    margin-bottom: 4px;
    border-bottom: 1px solid var(--line);
  }
  .badge {
    display: grid;
    place-items: center;
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--panel-2);
    border: 1px solid var(--key-edge);
    font-size: 11px;
    font-weight: 600;
  }
  .names {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .names b {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .names span {
    color: var(--text-dim);
    font-size: 11px;
  }
  .item {
    all: unset;
    box-sizing: border-box;
    padding: 6px 8px;
    border-radius: 3px;
    color: var(--text);
    cursor: pointer;
  }
  .item:hover,
  .item:focus-visible {
    background: var(--panel-2);
    outline: none;
    box-shadow: inset 0 0 0 1px var(--focus);
  }
  @media (pointer: coarse) {
    .pill {
      min-width: 32px;
      height: 32px;
      border-radius: 16px;
    }
    .item {
      padding: 12px 10px;
    }
  }
</style>
