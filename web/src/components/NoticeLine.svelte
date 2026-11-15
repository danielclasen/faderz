<script lang="ts">
  // The newest notice in a line of fixed height, with its repeat count
  // and the number of others; never an overlay on the controls. A notice
  // that a control is held carries ASK, which asks its holder for it; a
  // request from another session for a control this one holds carries
  // RELEASE.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { TRANSMITTING } from '../lib/sync/machines';
  import { tip } from '../lib/tip';
  const mixer = getMixer();
  const latest = $derived(mixer.notices.at(-1) ?? null);
  const others = $derived(mixer.notices.length - 1);
  /** The control a held-control notice names, while another session still holds it. */
  const askable = $derived(
    latest?.kind === 'conflict' && latest.key !== undefined && mixer.ownedByOther(latest.key) && TRANSMITTING.has(mixer.status.link) ? latest.key : null,
  );
  /** The control a request notice asks for, while this session still holds it. */
  const releasable = $derived(latest?.kind === 'request' && latest.path !== undefined && mixer.isMine(mixer.ownerOf(latest.path)) ? latest.path : null);
</script>

<div class="notice-line" role="log" aria-live="polite" aria-label="Notices">
  {#if latest}
    <!-- A text cut off at the end is whole in the tooltip, which focus
         shows too. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="notice {latest.kind}" data-testid="notice" tabindex="0" use:tip={latest.text}>
      <span class="text">{latest.text}</span>
      <span class="count num" class:shown={latest.count > 1}>x{latest.count}</span>
      <span class="more num" class:shown={others > 0}>+{others}</span>
      {#if latest.kind === 'conflict'}
        <!-- Kept in place while hidden, so the text does not move. -->
        <button
          class="ask"
          class:shown={askable !== null}
          onclick={() => askable && mixer.requestControl(askable)}
          aria-hidden={askable === null}
          title="Ask the holder to give the control up">ASK</button
        >
      {:else if latest.kind === 'request'}
        <button
          class="ask"
          class:shown={releasable !== null}
          onclick={() => releasable && mixer.giveUp(releasable)}
          aria-hidden={releasable === null}
          title="Give the control up: a talk ends, anything else is released">RELEASE</button
        >
      {/if}
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
  .notice:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .notice.conflict {
    border-left-color: var(--owned);
  }
  .notice.error {
    border-left-color: var(--err);
  }
  .notice.request {
    border-left-color: var(--accent);
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
  .notice .ask {
    visibility: hidden;
    height: 16px;
    line-height: 16px;
    padding: 0 6px;
    border: 1px solid var(--key-edge);
    border-radius: 2px;
    color: var(--text);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.06em;
  }
  .notice .ask.shown {
    visibility: visible;
  }
  .notice button {
    all: unset;
    cursor: pointer;
    color: var(--text-dim);
    padding: 0 4px;
  }
</style>
