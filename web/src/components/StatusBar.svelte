<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { sessionTag } from '../lib/sync/session';
  const mixer = getMixer();
  const counters = $derived(Object.entries(mixer.counters));
</script>

<footer class="bar">
  <span class="label">counters {mixer.countersReplica ?? '-'}</span>
  {#each counters as [name, value] (name)}
    <span class="c" class:nz={value > 0}><span>{name.replace(/_total$/, '')}</span> <b class="num">{value}</b></span>
  {/each}
  <span class="right num" title="Session and primary endpoint" data-testid="session">{sessionTag(mixer.status.session)} {mixer.endpoint ?? ''}</span>
</footer>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    height: 24px;
    padding: 0 10px;
    border-top: 1px solid var(--line);
    background: var(--panel);
    font-size: 10.5px;
    color: var(--text-faint);
    overflow: hidden;
    white-space: nowrap;
  }
  .label {
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }
  .c b {
    color: var(--text-dim);
    font-weight: 400;
  }
  .c.nz b {
    color: var(--warn);
  }
  .right {
    margin-left: auto;
  }
</style>
