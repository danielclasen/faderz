<script lang="ts">
  // Shown while the serving replica cannot commit: audio keeps running on
  // the last committed state, but nothing changed here takes effect. The
  // controls stay usable (with a ready standby the client fails over), so
  // the banner sits in a header slot that exists whether shown or not.
  // A slot too narrow for it cuts the text off; the whole text is in the
  // tooltip, which focus shows too.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { tip } from '../lib/tip';
  const mixer = getMixer();
  const status = $derived(mixer.status);
  const serving = $derived(status.replicas.find((r) => r.id === status.primaryReplica));
  const down = $derived(serving !== undefined && !serving.commitable);
  const pending = $derived(status.outbox > 0 ? `${status.outbox} pending` : '');
  const text = $derived(
    `CHANGES NOT COMMITTING - audio continues${pending ? `, ${pending}` : ''}. The serving replica has no quorum: changes are not committed. Audio continues on the last committed state.`,
  );
</script>

{#if down}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="banner" role="status" data-testid="commit-banner" tabindex="0" use:tip={text}>
    <span class="text"><b>CHANGES NOT COMMITTING</b> <span>audio continues</span></span>
    <span class="num fixed">{pending}</span>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 30px;
    padding: 0 10px;
    border-radius: 3px;
    background: var(--err);
    color: #fff;
    font-size: 12px;
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
    letter-spacing: 0.08em;
  }
</style>
