<script lang="ts">
  // A replica serves a newer faderz build than the one this page runs: a
  // rolled set's new UI and API. An UPDATE key in the header, its place
  // kept while no newer build is known so nothing moves when it shows,
  // with a dot until LATER; a press opens what it is about, with a reload.
  // Nothing opens or moves unless the operator asks. Below 430 px the
  // account pill stands in for it (Account.svelte).
  import { getMixer } from '../lib/state/mixer.svelte';
  import { update } from '../lib/update.svelte';

  const mixer = getMixer();
  let open = $state(false);
  let reloading = $state(false);
  /** A newer version a replica serves; the key shows while there is one. */
  const version = $derived(update.version(mixer.status.build));
  /** The dot: a newer version not put off. */
  const fresh = $derived(update.fresh(mixer.status.build));

  function reload(): void {
    // Focus stays on the key while the page reloads; a second press does nothing.
    if (reloading) return;
    reloading = true;
    location.reload();
  }
</script>

<div class="update" class:shown={version !== null}>
  <button
    class="key update-key"
    aria-label={fresh ? 'Update: a new faderz version is running' : 'Update'}
    aria-expanded={open}
    aria-controls="update-panel"
    aria-hidden={version === null}
    tabindex={version === null ? -1 : 0}
    data-testid="update-key"
    onclick={() => (open = !open)}
    ><i class="dot" class:on={fresh} aria-hidden="true" data-testid="update-dot"></i>UPDATE</button
  >
  <div class="panel" class:open={open && version !== null} id="update-panel" role="region" aria-label="Update" data-testid="new-version">
    <span>A new faderz version is running ({version}): reload to update.</span>
    <div class="actions">
      <button class="key" aria-disabled={reloading} aria-busy={reloading} onclick={reload}>{reloading ? 'RELOADING...' : 'RELOAD'}</button>
      <button
        class="key"
        onclick={() => {
          update.later = version;
          open = false;
        }}>LATER</button
      >
    </div>
  </div>
</div>

<style>
  .update {
    flex: none;
    visibility: hidden;
  }
  .update.shown {
    visibility: visible;
  }
  /* The narrowest header has no room for the key: the account pill
     carries the dot and its menu the update. */
  @media (max-width: 429px) {
    .update {
      display: none;
    }
  }
  .update-key {
    position: relative;
  }
  /* A dot in the key's corner, over its edge: it takes no room. */
  .dot {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    box-shadow: 0 0 0 1px var(--panel-2);
    visibility: hidden;
  }
  .dot.on {
    visibility: visible;
  }
  .panel {
    display: none;
    position: fixed;
    top: calc(var(--header-h) + 2px);
    right: 10px;
    z-index: 40;
    width: min(320px, calc(100vw - 20px));
    padding: 10px;
    flex-direction: column;
    gap: 10px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    color: var(--text);
    font-size: 13px;
  }
  .panel.open {
    display: flex;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .actions .key {
    min-height: 32px;
    padding: 0 12px;
  }
  @media (pointer: coarse) {
    .actions .key {
      min-height: 44px;
    }
  }
</style>
