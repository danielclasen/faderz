<script lang="ts">
  // Shown while this client holds a revision the cluster never committed,
  // or a history (epoch) the cluster no longer has after a restore from a
  // backup or an empty start. The client
  // never resyncs on its own then; the operator takes the cluster state.
  // An overlay over the top of the view: it moves nothing below it.
  import { getMixer } from '../lib/state/mixer.svelte';

  const mixer = getMixer();
  const ahead = $derived(mixer.status.link === 'Ahead' ? mixer.status.ahead : null);
</script>

{#if ahead}
  <div class="ahead" role="alert" data-testid="ahead">
    <b>AHEAD OF CLUSTER</b>
    <span>
      {#if ahead.otherHistory}
        This screen shows revision <span class="num">{ahead.lastRevision}</span> of an earlier history; the cluster was
        restored or started empty and is at revision <span class="num">{ahead.cluster}</span> of a new one.
      {:else}
        This screen shows revision <span class="num">{ahead.lastRevision}</span>; the cluster has only
        <span class="num">{ahead.cluster}</span>.
      {/if}
      Audio follows the cluster. Changes made here are held until you take the cluster state.
    </span>
    <button class="key take" onclick={() => mixer.adopt()}>TAKE CLUSTER STATE</button>
  </div>
{/if}

<style>
  .ahead {
    position: absolute;
    inset: 0 0 auto 0;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: var(--warn);
    color: #1a1200;
    font-size: 13px;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.4);
  }
  b {
    letter-spacing: 0.08em;
    white-space: nowrap;
  }
  span {
    flex: 1;
    min-width: 0;
  }
  .num {
    font-weight: 600;
  }
  .take {
    flex: none;
    height: 32px;
    padding: 0 12px;
    background: #1a1200;
    color: var(--warn);
    border-color: #1a1200;
  }
</style>
