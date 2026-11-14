<script lang="ts">
  // The newest notice in a line of fixed height, with its repeat count
  // and the number of others; never an overlay on the controls.
  import { getMixer } from '../lib/state/mixer.svelte';
  const mixer = getMixer();
  const latest = $derived(mixer.notices.at(-1) ?? null);
  const others = $derived(mixer.notices.length - 1);
</script>

<div class="notice-line" role="log" aria-live="polite" aria-label="Notices">
  {#if latest}
    <div class="notice {latest.kind}" data-testid="notice" title={latest.text}>
      <span class="text">{latest.text}</span>
      <span class="count num" class:shown={latest.count > 1}>x{latest.count}</span>
      <span class="more num" class:shown={others > 0}>+{others}</span>
      <button onclick={() => mixer.dismiss(latest.id)} aria-label="Dismiss notice">x</button>
    </div>
  {/if}
</div>

<style>
  .notice-line {
    height: 20px;
    min-width: 0;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    padding: 0 6px;
    border-radius: 3px;
    border-left: 3px solid var(--afl);
    background: var(--panel-2);
    color: var(--text);
    font-size: 11.5px;
  }
  .notice.conflict {
    border-left-color: var(--owned);
  }
  .notice.error {
    border-left-color: var(--err);
  }
  .text {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count,
  .more {
    visibility: hidden;
    width: 4ch;
    color: var(--text-dim);
  }
  .shown {
    visibility: visible;
  }
  .notice button {
    all: unset;
    cursor: pointer;
    color: var(--text-dim);
    padding: 0 4px;
  }
</style>
