<script lang="ts">
  // About / What's new: what the serving replica was built from and the
  // faderz changelog of that release, as GET /about answers it from the
  // binary. A modal dialog: Escape, the close key or a press outside it
  // closes it, and focus goes back to what opened it.
  import { about, latest, type Load } from '../lib/about.svelte';
  import type { About } from '../lib/contract/types';
  import { parseMarkdown, plainText, type Inline } from '../lib/markdown';
  import { fetchAbout } from '../lib/state/actions';

  let dialog = $state<HTMLDialogElement>();
  let load = $state<Load<About>>({ kind: 'busy' });
  /** The changelog's own title is the dialog's: its releases start at level 2. */
  const blocks = $derived(load.kind === 'done' ? parseMarkdown(load.value.changelog).filter((b) => b.kind !== 'heading' || b.level > 1) : []);
  /** A samplez release has notes of its own where a tag names it. */
  const samplezNotes = $derived(
    load.kind === 'done' && !load.value.build.samplez.includes('+') ? `https://github.com/danielclasen/samplez/releases/tag/v${load.value.build.samplez}` : null,
  );

  // Only the latest load shows: an older one ending late sets nothing.
  const loader = latest(fetchAbout, (l) => (load = l));
  const fetchIt = loader.run;

  $effect(() => {
    if (!dialog) return;
    if (about.open && !dialog.open) {
      dialog.showModal();
      // Asked each time it opens: a failover may have moved the console to
      // a replica of another build.
      void fetchIt();
    } else if (!about.open && dialog.open) {
      loader.cancel();
      dialog.close();
    }
  });

  /** A press on the backdrop, outside the dialog's box, closes it. */
  function outside(e: MouseEvent): void {
    if (e.target !== dialog) return;
    const r = dialog.getBoundingClientRect();
    if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) about.open = false;
  }
</script>

{#snippet runs(list: Inline[])}{#each list as r, i (i)}{#if r.kind === 'text'}{r.text}{:else if r.kind === 'bold'}<strong>{@render runs(r.children)}</strong>{:else if r.kind === 'code'}<code>{r.text}</code>{:else}<a href={r.href} target="_blank" rel="noopener noreferrer">{@render runs(r.children)}</a>{/if}{/each}{/snippet}

<dialog bind:this={dialog} class="about" aria-labelledby="about-title" data-testid="about" onclose={() => (about.open = false)} onclick={outside}>
  <div class="head">
    <h2 id="about-title">About / What's new</h2>
    <button class="close" aria-label="Close" data-testid="about-close" onclick={() => (about.open = false)}>&times;</button>
  </div>
  <div class="body" aria-busy={load.kind === 'busy'}>
    {#if load.kind === 'busy'}
      <p class="state" role="status" data-testid="about-busy">Loading the changelog...</p>
    {:else if load.kind === 'error'}
      <div class="state error" role="alert" data-testid="about-error">
        <p>The changelog could not be loaded: {load.message}</p>
        <button class="retry" onclick={() => void fetchIt()}>Try again</button>
      </div>
    {:else}
      <p class="build num" data-testid="about-build">
        faderz {load.value.build.faderz}, built against samplez
        {#if samplezNotes}<a href={samplezNotes} target="_blank" rel="noopener noreferrer">{load.value.build.samplez}</a>{:else}{load.value.build.samplez}{/if}
      </p>
      <div class="log" data-testid="about-changelog">
        {#each blocks as b, i (i)}
          {#if b.kind === 'heading'}
            <svelte:element this={`h${Math.min(6, b.level + 1)}`} class:breaking={/BREAKING CHANGES/i.test(plainText(b.children))}>{@render runs(b.children)}</svelte:element>
          {:else if b.kind === 'list'}
            <ul>
              {#each b.items as item, j (j)}<li>{@render runs(item)}</li>{/each}
            </ul>
          {:else}
            <p>{@render runs(b.children)}</p>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</dialog>

<style>
  .about {
    box-sizing: border-box;
    width: min(720px, calc(100vw - 32px));
    max-height: min(80vh, calc(100dvh - 32px));
    padding: 0;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    color: var(--text);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
  }
  .about[open] {
    display: flex;
    flex-direction: column;
  }
  .about::backdrop {
    background: rgb(0 0 0 / 0.6);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 8px 8px 16px;
    border-bottom: 1px solid var(--line);
  }
  h2 {
    flex: 1;
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .close {
    appearance: none;
    width: 28px;
    height: 28px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel-2);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .body {
    overflow-y: auto;
    overscroll-behavior: contain;
    padding: 12px 16px 16px;
  }
  .state,
  .build {
    margin: 0;
    color: var(--text-dim);
  }
  .error {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    color: var(--mute);
  }
  .error p {
    margin: 0;
  }
  .retry {
    padding: 6px 12px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel-2);
    cursor: pointer;
  }
  .log {
    overflow-wrap: anywhere;
    line-height: 1.45;
  }
  .log :global(h3) {
    margin: 18px 0 4px;
    font-size: 14px;
    font-weight: 600;
  }
  /* Features, Bug Fixes: section labels in the text colour, set apart
     by case and tracking rather than by size. */
  .log :global(h4) {
    margin: 12px 0 2px;
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  /* The one an operator must not miss. */
  .log :global(h4.breaking) {
    color: var(--mute);
    font-weight: 700;
  }
  .log :global(ul) {
    margin: 0;
    padding-left: 18px;
  }
  .log :global(p) {
    margin: 6px 0;
  }
  .about a {
    color: var(--focus);
  }
  .log :global(code) {
    font-family: var(--font-num);
  }
  @media (pointer: coarse) {
    .close {
      width: 44px;
      height: 44px;
    }
    .retry {
      padding: 12px 16px;
    }
  }
</style>
