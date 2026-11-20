<script lang="ts">
  // A replica serves a newer faderz build than the one this page runs: a
  // rolled set's new UI and API. A bar of its own between the header and
  // the view, covering nothing, with a reload; dismissed, it stays away
  // until a replica reports yet another build. The slot is always there,
  // empty but for that, so the app's grid rows stay where they are.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { newVersion } from '../lib/newVersion';

  const mixer = getMixer();
  let dismissed = $state<string | null>(null);
  let reloading = $state(false);
  const version = $derived(newVersion(mixer.status.build, dismissed));

  function reload(): void {
    // Focus stays on the key while the page reloads; a second press does nothing.
    if (reloading) return;
    reloading = true;
    location.reload();
  }
</script>

<div class="slot">
{#if version}
  <div class="new-version" role="status" data-testid="new-version">
    <span>A new faderz version is running ({version}): reload to update.</span>
    <button class="key reload" aria-disabled={reloading} aria-busy={reloading} onclick={reload}>{reloading ? 'RELOADING...' : 'RELOAD'}</button>
    <button class="key dismiss" onclick={() => (dismissed = version)}>LATER</button>
  </div>
{/if}
</div>

<style>
  .new-version {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    padding: 6px 12px;
    background: var(--panel-2);
    color: var(--text);
    border-bottom: 1px solid var(--accent, var(--warn));
    font-size: 13px;
  }
  span {
    flex: 1;
    min-width: 12em;
  }
  .key {
    flex: none;
    min-height: 32px;
    padding: 0 12px;
  }
  @media (pointer: coarse) {
    .key {
      min-height: 44px;
    }
  }
</style>
