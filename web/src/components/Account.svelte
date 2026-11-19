<script lang="ts">
  // The account pill at the header's right end, at every width and in
  // every sign-in mode: the user's initials, or a person mark where
  // nobody is signed in. It opens a menu that names the role, and the
  // user only where that is no email address; its entries are a list of
  // settings (the theme), then Sign out where the mode signs out; under
  // them, what the serving replica was built from, an entry that opens
  // About / What's new.
  import { tick } from 'svelte';
  import { getAuth } from '../lib/auth.svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { follow, type Placement } from '../lib/anchor';
  import { initials, isEmail } from '../lib/account';
  import { setTheme, theme } from '../lib/theme.svelte';
  import { openAbout } from '../lib/about.svelte';

  const mixer = getMixer();
  // Absent where the header is mounted without the sign-in state.
  const auth = getAuth() as ReturnType<typeof getAuth> | undefined;
  const session = $derived(auth?.session ?? null);
  /** Who is signed in, where a mode names anybody or has a role to show. */
  const user = $derived(session?.role && session.mode !== 'none' ? (session.user ?? 'token') : null);
  /** The user's name where it may be shown: never an email address. */
  const name = $derived(user !== null && !isEmail(user) ? user : null);
  const label = $derived(`Account${name ? ` ${name}` : ''}${session?.role ? `, ${session.role}` : ''}`);

  interface Entry {
    id: string;
    label: string;
    /** Shown while `run` has not settled; an entry with it keeps the menu open meanwhile. */
    busyLabel?: string;
    run: () => void | Promise<void>;
  }
  const entries = $derived<Entry[]>([
    {
      id: 'theme',
      label: `Switch to ${theme.value === 'dark' ? 'light' : 'dark'} theme`,
      run: () => setTheme(theme.value === 'dark' ? 'light' : 'dark'),
    },
    ...(session?.role != null && (session.mode === 'token' || session.mode === 'oidc') ? [{ id: 'sign-out', label: 'Sign out', busyLabel: 'Signing out...', run: leave }] : []),
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
  /** The entry whose action runs now: it shows so and takes no second press. */
  let busy = $state<string | null>(null);

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
    if (busy !== null) return;
    if (!entry.busyLabel) {
      hide(true);
      return void (await entry.run());
    }
    busy = entry.id;
    try {
      await entry.run();
    } finally {
      busy = null;
      // Back to the pill only from the menu: closed meanwhile, the focus stays where it went.
      const from = menu?.contains(document.activeElement) ?? false;
      if (open) hide(from);
    }
  }
</script>

<svelte:window onpointerdown={outside} />

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
    onkeydown={pillKey}
    >{#if user !== null}<span class="fixed">{initials(user)}</span>{:else}{@render person()}{/if}</button
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
        <span class="badge fixed" aria-hidden="true">{#if user !== null}{initials(user)}{:else}{@render person()}{/if}</span>
        <span class="names">
          <b>{name ?? (user !== null ? 'Signed in' : 'Not signed in')}</b>
          {#if session?.role}<span data-testid="account-role">{session.role}</span>{/if}
        </span>
      </div>
      {#each entries as entry (entry.id)}
        <button
          class="item"
          role="menuitem"
          tabindex="-1"
          data-entry={entry.id}
          aria-busy={busy === entry.id}
          aria-disabled={busy !== null}
          onclick={() => choose(entry)}>{busy === entry.id && entry.busyLabel ? entry.busyLabel : entry.label}</button
        >
      {/each}
      {#if mixer.status.build}
        <button
          class="item build num"
          role="menuitem"
          tabindex="-1"
          data-testid="account-build"
          aria-label={`About / What's new: faderz ${mixer.status.build.faderz}, samplez ${mixer.status.build.samplez}`}
          onclick={() => choose({ id: 'about', label: "About / What's new", run: openAbout })}
        >
          <span>faderz {mixer.status.build.faderz}</span>
          <span>samplez {mixer.status.build.samplez}</span>
        </button>
      {/if}
    </div>
  {/if}

{#snippet person()}
  <svg class="person" viewBox="0 0 12 12" aria-hidden="true"><circle cx="6" cy="4" r="2.3" /><path d="M1.6 11.2c0-2.6 2-4.2 4.4-4.2s4.4 1.6 4.4 4.2" /></svg>
{/snippet}

<style>
  .pill {
    flex: none;
    display: grid;
    place-items: center;
    box-sizing: border-box;
    /* One width for the person mark and any two initials, so the
       session's arrival moves nothing. */
    width: 32px;
    height: var(--item-h, 26px);
    padding: 0;
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
  .person {
    width: 12px;
    height: 12px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.4;
  }
  .item[aria-busy='true'] {
    color: var(--text-dim);
    cursor: progress;
  }
  .item[aria-disabled='true']:not([aria-busy='true']) {
    opacity: 0.5;
    cursor: default;
  }
  .item:hover,
  .item:focus-visible {
    background: var(--panel-2);
    outline: none;
    box-shadow: inset 0 0 0 1px var(--focus);
  }
  .build {
    display: flex;
    flex-direction: column;
    margin-top: 4px;
    border-top: 1px solid var(--line);
    border-radius: 0 0 3px 3px;
    color: var(--text-faint);
    font-size: 10px;
    line-height: 14px;
  }
  @media (pointer: coarse) {
    .pill {
      border-radius: 16px;
    }
    .item {
      padding: 12px 10px;
    }
  }
</style>
