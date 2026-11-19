<script lang="ts">
  import { fit } from '../lib/format';
  import { onMount } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { getMixer } from '../lib/state/mixer.svelte';
        
  const mixer = getMixer();
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  /** A short screen (App.svelte): the pending count takes less of the one header row. */
  const short = new MediaQuery('max-height: 500px');
  const status = $derived(mixer.status);
  const failover = $derived(
    status.failover && mixer.failoverAt !== null && now - mixer.failoverAt < 8000 ? status.failover : null,
  );
</script>

<!-- Every slot has a fixed size and keeps it when its text changes or
     empties. The link lamp, the quorum and the replica rows are in
     ConnectionKey. -->
<div class="status" aria-label="Connection" data-testid="status">
  <span class="slot pend">
    <span class="box">
      {#if status.outbox > 0}
        <span class="pending fixed" data-testid="pending" title={`${status.outbox} commands waiting for confirmation`}>{#if short.current}{fit(String(status.outbox), 4)}<span class="sr-only">{' pending'}</span>{:else}{fit(`${status.outbox} pending`, 11)}{/if}</span>
      {/if}
    </span>
  </span>
  <span class="slot fo">
    {#if failover}
      <span class="failover fixed" role="status" data-testid="failover" title={`Failover from ${failover.from ?? '-'} to ${failover.to ?? '-'} after ${failover.reason}`}
        >{`FAILOVER ${failover.from ?? '-'}->${failover.to ?? '-'}`}</span
      >
    {/if}
  </span>
  <span class="sr-only" data-testid="revision"><span class="rev">rev {status.revision}</span></span>
</div>

<style>
  /* Items of the header's status group (App.svelte). */
  .status {
    display: contents;
  }
  .slot {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    white-space: nowrap;
    font-size: 11px;
  }
  /* The count comes and goes with every write. A flex item is never a
     relayout boundary, so it sits in a block child of fixed size with
     size and layout containment: a change lays out the box, not the
     page. Paint is not contained, so a count wider than the slot still
     shows in full rather than clipped. */
  .slot.pend {
    display: block;
    width: 11ch;
  }
  .slot.pend .box {
    display: block;
    height: 14px;
    line-height: 14px;
    text-align: center;
    contain: size layout style;
  }
  /* Sized in the chip's own monospace digits, its padding included, for
     FAILOVER and two ids of up to three characters; longer ids are cut
     with an ellipsis and the title names both. Left-aligned: the target
     appearing after the source moves nothing. */
  .slot.fo {
    justify-content: flex-start;
    font-family: var(--font-num);
    width: calc(15ch + 16px);
  }
  .failover {
    box-sizing: border-box;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    height: var(--item-h, 24px);
    line-height: var(--item-h, 24px);
    padding: 0 8px;
    border-radius: 3px;
    background: var(--warn);
    color: #1a1200;
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }
  /* The revision changes with every patch. The visually hidden box is
     positioned, which is never a relayout boundary, so the text sits in
     a block child with strict containment: a change lays out that box,
     not the page. */
  .rev {
    display: block;
    height: 1px;
    contain: strict;
  }
  .pending {
    font-size: 10.5px;
    color: var(--warn);
  }
  @media (max-width: 1500px) {
    .slot.fo {
      display: none;
    }
  }
  @media (max-width: 700px) {
    .slot.pend {
      display: none;
    }
  }
  @media (max-height: 500px) {
    .slot.pend,
    .pending {
      width: 4ch;
    }
  }
</style>
