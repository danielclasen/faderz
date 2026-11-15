<script lang="ts">
  // Shown in the notice line, in place of the notices, while the serving
  // replica cannot commit: audio runs on the last committed state, and
  // nothing changed here takes effect until a replica commits it. The
  // controls stay usable; keys pressed meanwhile show their state
  // desaturated until it commits (MixerStore.pendingKey). The line has a
  // fixed height whether it shows notices, this or nothing. A narrower
  // line drops "audio continues" below 340 px and "CHANGES" and
  // "pending" below 240 px; the whole text is in the tooltip.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { tip } from '../lib/tip';
  const mixer = getMixer();
  const outbox = $derived(mixer.status.outbox);
  const text = $derived(`Changes not committing - audio continues on the last committed state. ${outbox} pending: sent again until a replica commits them or 3 s pass.`);
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="banner" role="alert" data-testid="commit-banner" tabindex="0" use:tip={text} aria-label={text}>
  <span class="text"><b><span class="long">CHANGES{' '}</span>NOT COMMITTING</b><span class="tail">{' '}- audio continues</span></span>
  <span class="count num" data-testid="commit-pending">{outbox}<span class="long">{' '}pending</span></span>
</div>

<style>
  .banner {
    container-type: inline-size;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    padding: 0 6px;
    border-radius: 3px;
    background: var(--err);
    color: #fff;
    font-size: 11.5px;
    white-space: nowrap;
    overflow: hidden;
  }
  .banner:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  b {
    letter-spacing: 0.06em;
  }
  .count {
    flex: none;
    min-width: 2ch;
    padding: 0 4px;
    border-radius: 2px;
    background: rgb(0 0 0 / 0.25);
    text-align: right;
  }
  @container (width < 340px) {
    .tail {
      display: none;
    }
  }
  @container (width < 240px) {
    .long {
      display: none;
    }
  }
  @container (width < 160px) {
    .text {
      font-size: 10px;
    }
    b {
      letter-spacing: 0;
    }
  }
</style>
