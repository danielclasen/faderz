<script lang="ts">
  // Shown while the serving replica cannot commit: audio keeps running on
  // the last committed state, but nothing changed here takes effect. The
  // controls stay usable (with a ready standby the client fails over), so
  // the banner sits in a header slot that exists whether shown or not.
  import { getMixer } from '../lib/state/mixer.svelte';
  const mixer = getMixer();
  const status = $derived(mixer.status);
  const serving = $derived(status.replicas.find((r) => r.id === status.primaryReplica));
  const down = $derived(serving !== undefined && !serving.commitable);
</script>

{#if down}
  <div class="banner" role="alert" data-testid="commit-banner" title="The serving replica has no quorum: changes are not committed. Audio continues on the last committed state.">
    <b>CHANGES NOT COMMITTING</b>
    <span>audio continues</span>
    <span class="num fixed">{status.outbox > 0 ? `${status.outbox} pending` : ''}</span>
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
  b {
    letter-spacing: 0.08em;
  }
</style>
