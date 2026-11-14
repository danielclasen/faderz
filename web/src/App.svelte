<script lang="ts">
  import { MediaQuery } from 'svelte/reactivity';
  import { getMixer } from './lib/state/mixer.svelte';
  import { setTheme, type Theme } from './lib/theme';
  import MixerView from './components/MixerView.svelte';
  import RoutingView from './components/RoutingView.svelte';
  import AuxView from './components/AuxView.svelte';
  import AfvView from './components/AfvView.svelte';
  import ScenesView from './components/ScenesView.svelte';
  import SetupView from './components/SetupView.svelte';
  import Detail from './components/Detail.svelte';
  import StatusArea from './components/StatusArea.svelte';
  import Consoles from './components/Consoles.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import AheadBanner from './components/AheadBanner.svelte';
  import { tip } from './lib/tip';
  import NoticeLine from './components/NoticeLine.svelte';
  import LockKey from './components/LockKey.svelte';
  import Users from './components/Users.svelte';
  import CommitBanner from './components/CommitBanner.svelte';

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
  const commitable = $derived(mixer.status.replicas.find((r) => r.id === mixer.status.primaryReplica)?.commitable ?? true);

  const VIEWS = [
    { id: 'mixer', label: 'Mixer' },
    { id: 'routing', label: 'Routing' },
    { id: 'aux', label: 'Aux / N-1' },
    { id: 'afv', label: 'AFV' },
    { id: 'scenes', label: 'Scenes' },
    { id: 'setup', label: 'Setup' },
  ] as const;
  type View = (typeof VIEWS)[number]['id'];
  let view = $state<View>('mixer');
  let selected = $state<string | null>(null);
  /** Processing section to show first when a strip's chip selected the channel. */
  let section = $state<string | null>(null);
  let theme = $state<Theme>((document.documentElement.dataset['theme'] as Theme) ?? 'dark');

  /** Moves focus to a region; on a narrow screen the masters open first. */
  function skip(e: MouseEvent, id: string): void {
    e.preventDefault();
    document.dispatchEvent(new CustomEvent('faderz:show-masters'));
    requestAnimationFrame(() => document.getElementById(id)?.focus());
  }
  function toggleTheme(): void {
    theme = theme === 'dark' ? 'light' : 'dark';
    setTheme(theme);
  }
  /** Selecting never closes: a second press on the selected strip keeps it; CLOSE or Escape closes. */
  function select(id: string, to: string | null = null): void {
    selected = id;
    section = to;
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
    <div class="right">
      <!-- Free header space: the commit banner when it is up, otherwise
           the notices on a short screen. It grows into the room the fixed
           items leave, so what it shows moves none of them. -->
      <div class="mid">
        {#if mixer.shape && !commitable}<CommitBanner />{:else if short.current}<NoticeLine />{/if}
      </div>
      <Consoles />
      <StatusArea />
      <Users />
      <!-- Reserved: shown or not, the tone lamp moves nothing. -->
      <span class="tone-slot">
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <span class="tone" class:on={toneOn} role="status" data-testid="tone" aria-hidden={!toneOn} use:tip={toneOn ? toneText : ''} tabindex={toneOn ? 0 : -1}
          >TONE<span class="sr-only">{toneOn ? ` on: ${toneText}` : ''}</span></span
        >
      </span>
      <LockKey />
      <button class="key" onclick={toggleTheme} aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`}>
        {theme === 'dark' ? 'LIGHT' : 'DARK'}
      </button>
    </div>
  </header>

  <main>
    <AheadBanner />
    <svelte:boundary onerror={(e) => console.error(e)}>
    {#if !mixer.shape || !mixer.state}
      <div class="waiting" role="status">Connecting to the mixer...</div>
    {:else if view === 'mixer'}
      <MixerView {selected} onselect={select} />
    {:else if view === 'routing'}
      <RoutingView />
    {:else if view === 'aux'}
      <AuxView />
    {:else if view === 'afv'}
      <AfvView />
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
    {#if view === 'mixer' && selected && mixer.state}
      {#key selected}
        <Detail id={selected} {section} onclose={() => (selected = null)} onselect={(id) => select(id)} />
      {/key}
    {/if}
  </main>


  <StatusBar />
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    height: 100%;
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
  /* Phones: the views are a menu, so the header stays one row with the
     connection state in view. */
  @media (max-width: 700px) {
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
  /* Short screens (a phone in landscape): one header row of 36 px with
     the view menu, the notices and the connection state; the status bar
     is gone, so the faders get the height. */
  @media (max-height: 500px) {
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
  .mid {
    flex: 1 1 0;
    min-width: 0;
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
  .tone-slot {
    flex: none;
    width: 46px;
    display: flex;
    justify-content: center;
  }
  .tone {
    visibility: hidden;
    height: 24px;
    line-height: 24px;
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
</style>
