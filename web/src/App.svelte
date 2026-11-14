<script lang="ts">
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
  import Toasts from './components/Toasts.svelte';
  import AheadBanner from './components/AheadBanner.svelte';

  const mixer = getMixer();

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
  let theme = $state<Theme>((document.documentElement.dataset['theme'] as Theme) ?? 'dark');

  function toggleTheme(): void {
    theme = theme === 'dark' ? 'light' : 'dark';
    setTheme(theme);
  }
  function select(id: string): void {
    selected = selected === id ? null : id;
  }
</script>

<div class="app">
  <header class="top">
    <div class="brand" aria-label="faderz"><i></i>faderz</div>
    <nav aria-label="Views">
      {#each VIEWS as v (v.id)}
        <button class="tab" aria-current={view === v.id ? 'page' : undefined} onclick={() => (view = v.id)}>{v.label}</button>
      {/each}
    </nav>
    <div class="right">
      <Consoles />
      <StatusArea />
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
  </main>

  {#if view === 'mixer' && selected && mixer.state}
    {#key selected}
      <Detail id={selected} onclose={() => (selected = null)} />
    {/key}
  {/if}

  <StatusBar />
  <Toasts />
</div>

<style>
  .app {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto auto;
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
    flex-wrap: wrap;
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
  .tab:hover {
    color: var(--text);
  }
  .tab[aria-current='page'] {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 4px 0;
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
