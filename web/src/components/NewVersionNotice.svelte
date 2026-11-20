<script lang="ts">
  // A replica serves a faderz build other than the one this page runs: a
  // rolled set's new UI and API. A note at the top of the view, over it
  // without moving it, with a reload; dismissed, it stays away until a
  // replica reports yet another build.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { newVersion } from '../lib/newVersion';

  const mixer = getMixer();
  let dismissed = $state<string | null>(null);
  let reloading = $state(false);
  const version = $derived(newVersion(mixer.status.build, dismissed));

  function reload(): void {
    reloading = true;
    location.reload();
  }
</script>

{#if version}
  <div class="new-version" role="status" data-testid="new-version">
    <span>A new faderz version is running ({version}): reload to update.</span>
    <button class="key reload" disabled={reloading} aria-busy={reloading} onclick={reload}>{reloading ? 'RELOADING...' : 'RELOAD'}</button>
    <button class="key dismiss" aria-label="Dismiss" onclick={() => (dismissed = version)}>LATER</button>
  </div>
{/if}

<style>
  .new-version {
    position: absolute;
    inset: 0 0 auto 0;
    z-index: 19;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    padding: 6px 12px;
    background: var(--panel-2);
    color: var(--text);
    border-bottom: 1px solid var(--accent, var(--warn));
    font-size: 13px;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.3);
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
