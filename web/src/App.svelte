<script lang="ts">
  import { MediaQuery } from 'svelte/reactivity';
  import { getMixer } from './lib/state/mixer.svelte';
  import MixerView from './components/MixerView.svelte';
  import RoutingView from './components/RoutingView.svelte';
  import AuxView from './components/AuxView.svelte';
  import AfvView from './components/AfvView.svelte';
  import DuckView from './components/DuckView.svelte';
  import DuckShortcuts from './components/DuckShortcuts.svelte';
  import ScenesView from './components/ScenesView.svelte';
  import SetupView from './components/SetupView.svelte';
  import StatusArea from './components/StatusArea.svelte';
  import ConnectionKey from './components/ConnectionKey.svelte';
  import Consoles from './components/Consoles.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import AheadBanner from './components/AheadBanner.svelte';
  import { tip } from './lib/tip';
  import NoticeLine from './components/NoticeLine.svelte';
  import LockKey from './components/LockKey.svelte';
  import Users from './components/Users.svelte';
  import TouchInfo from './components/TouchInfo.svelte';
  import SignIn from './components/SignIn.svelte';
  import Account from './components/Account.svelte';
  import AboutDialog from './components/AboutDialog.svelte';

  const mixer = getMixer();
  /** A phone in landscape: notices move from the status bar into the header. */
  const short = new MediaQuery('max-height: 500px');
  const toneOn = $derived(!!mixer.state?.tone.on && (mixer.state?.tone.targets.length ?? 0) > 0);
  const toneText = $derived.by(() => {
    const t = mixer.state?.tone;
    const shape = mixer.shape;
    if (!t || !shape) return '';
    const names = [...shape.buses, ...shape.auxes, ...shape.mixMinus].filter((e) => t.targets.includes(e.id)).map((e) => e.name);
    return `Line-up tone 1 kHz ${t.level} dBFS replaces ${names.join(', ')}`;
  });
  // Overs latch in every view, against the shared true-peak limit.
  $effect(() => {
    const max = mixer.state?.metering.zones.truePeakMax;
    if (max !== undefined) mixer.meters.truePeakMax = max;
  });

  const VIEWS = [
    { id: 'mixer', label: 'Mixer' },
    { id: 'routing', label: 'Routing' },
    { id: 'aux', label: 'Aux / N-1' },
    { id: 'afv', label: 'AFV' },
    { id: 'duck', label: 'Duck' },
    { id: 'scenes', label: 'Scenes' },
    { id: 'setup', label: 'Setup' },
  ] as const;
  type View = (typeof VIEWS)[number]['id'];
  let view = $state<View>('mixer');
  let selected = $state<string | null>(null);
  /** Processing section to show first when a strip's chip selected the channel. */
  let section = $state<string | null>(null);
  // Other screens mark what this one has selected.
  $effect(() => mixer.select(selected));

  /** Moves focus to a region; on a narrow screen the masters open first. */
  function skip(e: MouseEvent, id: string): void {
    e.preventDefault();
    document.dispatchEvent(new CustomEvent('faderz:show-masters'));
    requestAnimationFrame(() => document.getElementById(id)?.focus());
  }
  /** Selecting never closes: a second press on the selected strip keeps it; CLOSE or Escape closes. */
  function select(id: string, to: string | null = null): void {
    selected = id;
    section = to;
  }
  /** Closes the processing panel and gives focus back to the head of what it showed. */
  function close(): void {
    const id = selected;
    selected = null;
    if (id) requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-channel="${id}"] .head, [data-master="${id}"] .head`)?.focus({ preventScroll: true }));
  }
</script>

<div class="app">
  <!-- Skip links: shown when focused, over the header, moving nothing. -->
  {#if view === 'mixer'}
    <div class="skip" role="navigation" aria-label="Skip links">
      <a href="#mixer-masters" onclick={(e) => skip(e, 'mixer-masters')}>Skip to masters</a>
      <a href="#monitor" onclick={(e) => skip(e, 'monitor')}>Skip to monitor</a>
    </div>
  {/if}
  <header class="top">
    <div class="brand" aria-label="faderz"><i></i>faderz</div>
    <select class="view-menu" aria-label="View" value={view} onchange={(e) => (view = (e.currentTarget as HTMLSelectElement).value as View)}>
      {#each VIEWS as v (v.id)}<option value={v.id}>{v.label}</option>{/each}
    </select>
    <nav aria-label="Views">
      {#each VIEWS as v (v.id)}
        <button class="tab" aria-current={view === v.id ? 'page' : undefined} onclick={() => (view = v.id)}>{v.label}</button>
      {/each}
    </nav>
    <!-- Status left, controls right. Every status slot keeps its size
         whether it shows or not; the surfaces, whose number the shape
         sets, stand furthest left next to the free space, so they move
         nothing when they arrive. -->
    <div class="right">
      <!-- Free header space: the notice line on a short screen, which has
           no status bar. It grows into the room the fixed items leave, so
           what it shows moves none of them. -->
      <div class="mid">
        {#if short.current}<NoticeLine />{/if}
      </div>
      <div class="group status-group" role="group" aria-label="Status">
        <Consoles />
        <StatusArea />
        <!-- Reserved: shown or not, the tone lamp moves nothing. -->
        <span class="tone-slot">
          <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
          <span class="tone" class:on={toneOn} role="status" data-testid="tone" aria-hidden={!toneOn} use:tip={toneOn ? toneText : ''} tabindex={toneOn ? 0 : -1}
            >TONE<span class="sr-only">{toneOn ? ` on: ${toneText}` : ''}</span></span
          >
        </span>
        {#if short.current}<ConnectionKey where="header" />{/if}
      </div>
      <span class="sep" aria-hidden="true"></span>
      <div class="group controls" role="group" aria-label="Controls">
        <Users />
        <LockKey />
        <Account />
      </div>
    </div>
  </header>

  <main>
    <AheadBanner />
    <svelte:boundary onerror={(e) => console.error(e)}>
    {#if !mixer.shape || !mixer.state}
      <div class="waiting" role="status">Connecting to the mixer...</div>
    {:else if view === 'mixer'}
      <MixerView {selected} {section} onselect={select} onclose={close} />
    {:else if view === 'routing'}
      <RoutingView />
    {:else if view === 'aux'}
      <AuxView />
    {:else if view === 'afv'}
      <AfvView />
    {:else if view === 'duck'}
      <DuckView />
    {:else if view === 'scenes'}
      <ScenesView />
    {:else}
      <SetupView />
    {/if}
    {#snippet failed(error, reset)}
      <div class="failed" role="alert" data-testid="view-error">
        <strong>This view failed to render.</strong>
        <pre>{error instanceof Error ? `${error.name}: ${error.message}` : String(error)}</pre>
        <button class="key" onclick={reset}>RETRY</button>
      </div>
    {/snippet}
    </svelte:boundary>
  </main>


  {#if mixer.shape && mixer.state}<DuckShortcuts />{/if}
  <StatusBar />
  <TouchInfo />
  <SignIn />
  <AboutDialog />
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    height: 100%;
  }
  /* Height of the header, set beside each header layout: panels that
     open under it (StatusArea, Users, the dev mock key) start below it. */
  :global(:root) {
    --header-h: 44px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 14px;
    min-height: 44px;
    padding: 0 10px;
    background: var(--panel);
    border-bottom: 1px solid var(--line-strong);
    flex-wrap: nowrap;
    height: 44px;
    box-sizing: border-box;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 7px;
    font-family: var(--font-num);
    font-weight: 600;
    font-size: 15px;
    letter-spacing: -0.03em;
  }
  .brand i {
    width: 10px;
    height: 16px;
    border-radius: 2px;
    background: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 30%, transparent);
  }
  nav {
    display: flex;
    gap: 2px;
    overflow-x: auto;
    min-width: 0;
    flex: 0 1 auto;
    scrollbar-width: none;
  }
  .tab {
    appearance: none;
    border: none;
    background: none;
    color: var(--text-dim);
    font-weight: 600;
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 0 10px;
    height: 44px;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    white-space: nowrap;
  }
  .view-menu {
    display: none;
    height: 30px;
    font-size: 13px;
    font-weight: 600;
  }
  /* Below 1024 px the view tabs, the brand name and the right group do
     not fit one row: the views are a menu and the brand its mark, so the
     header stays one row, within the screen, with the connection state in
     view. */
  @media (max-width: 1023px) {
    nav {
      display: none;
    }
    .view-menu {
      display: block;
    }
    .brand {
      font-size: 0;
      gap: 0;
    }
  }
  /* Just above the menu width the seven view tabs fit the header row
     beside the right group only a little tighter. */
  @media (min-width: 1024px) and (max-width: 1199px) {
    .tab {
      padding: 0 6px;
      letter-spacing: 0.05em;
    }
  }
  /* Tablets in portrait: room for a second header row, which carries the
     view tabs, so a view stays one tap away. The rows have fixed heights
     at this size whatever happens, so nothing moves. */
  @media (orientation: portrait) and (min-width: 701px) and (max-width: 1023px) and (min-height: 501px) {
    :global(:root) {
      --header-h: 76px;
    }
    .top {
      display: grid;
      grid-template-columns: auto minmax(0, 1fr);
      grid-template-rows: 44px 32px;
      height: 76px;
      min-height: 76px;
      column-gap: 14px;
      row-gap: 0;
    }
    .view-menu {
      display: none;
    }
    nav {
      display: flex;
      grid-column: 1 / -1;
      grid-row: 2;
      margin: 0 -10px;
      padding: 0 4px;
      border-top: 1px solid var(--line);
    }
    .tab {
      height: 31px;
    }
  }
  /* Short screens (a phone in landscape): one header row of 36 px with
     the view menu, the notices and the connection state; the status bar
     is gone, so the faders get the height. */
  @media (max-height: 500px) {
    :global(:root) {
      --header-h: 36px;
    }
    .top {
      height: 36px;
      min-height: 36px;
      gap: 8px;
    }
    nav {
      display: none;
    }
    .view-menu {
      display: block;
    }
    .brand {
      font-size: 0;
      gap: 0;
    }
    .right {
      flex: 1 1 0;
      min-width: 0;
    }
  }
  .tab:hover {
    color: var(--text);
  }
  .tab[aria-current='page'] {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  /* The right group fills the rest of the header and packs its items at
     its end: an item that appears or grows at its start moves nothing. */
  .skip {
    position: absolute;
  }
  .skip a {
    position: absolute;
    left: -10000px;
    top: 4px;
    z-index: 100;
    padding: 6px 10px;
    border-radius: 3px;
    background: var(--panel);
    color: var(--text);
    border: 2px solid var(--focus);
  }
  .skip a:focus {
    left: 8px;
  }
  /* Sized by the header alone: without inline-size containment the
     unbroken width of a notice or banner counts in the header's
     min-content width, and a phone widens its layout viewport to fit. */
  .mid {
    flex: 1 1 0;
    min-width: 0;
    contain: inline-size;
  }
  .right {
    flex: 1 0 auto;
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
    flex-wrap: nowrap;
    overflow: hidden;
  }
  /* The status items, then the controls: groups to assistive technology,
     items of the right group to layout, so an item that appears at a
     group's start moves no box. */
  .group {
    display: contents;
  }
  /* Between what the header shows and what it does. */
  .sep {
    flex: none;
    width: 1px;
    height: 18px;
    background: var(--line-strong);
  }
  /* Every item of the header row is one height: 26 px, 32 px to a finger. */
  .top {
    --item-h: 26px;
  }
  @media (pointer: coarse) {
    .top {
      --item-h: 32px;
    }
  }
  .controls :global(.key) {
    height: var(--item-h);
  }
  .tone-slot {
    flex: none;
    width: 46px;
    display: flex;
    justify-content: center;
  }
  .tone {
    visibility: hidden;
    height: var(--item-h);
    line-height: var(--item-h);
    padding: 0 6px;
    border-radius: 3px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    background: var(--tone);
    color: var(--tone-ink);
  }
  .tone.on {
    visibility: visible;
    animation: tone-blink 1s steps(1) infinite;
  }
  .tone:focus-visible {
    outline: 2px solid var(--focus);
  }
  @keyframes tone-blink {
    50% {
      opacity: 0.55;
    }
  }
  main {
    position: relative;
    min-height: 0;
    overflow: hidden;
  }
  .failed {
    padding: 16px;
    color: var(--err);
  }
  .failed pre {
    white-space: pre-wrap;
    color: var(--text);
  }
  .waiting {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--text-dim);
  }
  /* Short screens and phones: narrower gaps on the right, so the notice
     line in the one header row has room for its text, and a locked desk's
     key and the user's pill fit beside the connection state. After the
     .right rule, which it overrides. */
  @media (max-height: 500px), (max-width: 700px) {
    .right {
      gap: 6px;
    }
    .tone-slot {
      width: 36px;
    }
    .tone {
      padding: 0 3px;
      letter-spacing: 0;
    }
  }
  /* The separator stands beside the surfaces: without them (none in
     the shape, or below 1301 px, where Consoles hides them and the users
     panel lists them) the status slots left of it are mostly empty.
     Before the state names them it keeps its room, so their arrival
     moves nothing. */
  .right:not(:has(:global([data-testid='consoles']))) .sep {
    visibility: hidden;
  }
  @media (max-width: 1300px) {
    .sep {
      display: none;
    }
  }
  /* The narrowest phones: the brand mark goes and the gaps narrow, so
     the view menu, the tone lamp, USR, a locked desk's key and the
     account pill fit 320 px. Last in the sheet: it overrides the rules
     above. */
  @media (max-width: 389px) {
    .top {
      gap: 4px;
      padding: 0 4px;
    }
    .brand {
      display: none;
    }
    .right {
      gap: 4px;
    }
    .tone-slot {
      width: 36px;
    }
    .view-menu {
      font-size: 11px;
    }
    .tone {
      padding: 0 3px;
      letter-spacing: 0;
    }
  }
</style>
