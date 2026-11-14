<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';

  const mixer = getMixer();
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });
  const recentFailover = $derived(mixer.failover && now - mixer.failover.at < 8000 ? mixer.failover : null);
  const linkText = $derived(mixer.link === 'live' ? 'LIVE' : mixer.link === 'syncing' ? 'SYNC' : 'RECONNECTING');
</script>

<div class="status" aria-label="Connection">
  <div class="link {mixer.link}" role="status" aria-live="polite" data-testid="link">
    <i></i><b>{linkText}</b>
    <span class="num rev" title="Global state revision" data-testid="revision">rev {mixer.revision}</span>
  </div>
  {#if recentFailover}
    <div class="failover" role="status" data-testid="failover">FAILOVER {recentFailover.from} -&gt; {recentFailover.to}</div>
  {/if}
  {#if mixer.pendingCount > 0}
    <span class="pending num" title="Commands waiting for confirmation">{mixer.pendingCount} pending</span>
  {/if}
  <div class="replicas" aria-label="Replicas" data-testid="replicas">
    {#each mixer.replicas as r (r.id)}
      <span
        class="replica"
        class:serving={mixer.replica?.id === r.id}
        class:down={!r.healthy}
        title={`${r.id}: ${r.role}, ${r.healthy ? 'healthy' : 'unhealthy'}${mixer.replica?.id === r.id ? ', serving this client' : ''}`}
        data-replica={r.id}
      >
        {r.id}{#if r.role === 'leader'}<sup>L</sup>{/if}
      </span>
    {/each}
  </div>
</div>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .link {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 8px;
    height: 24px;
    border-radius: 3px;
    background: var(--well);
    color: #e6edf3;
    font-size: 11px;
    letter-spacing: 0.08em;
  }
  .link i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
    box-shadow: 0 0 6px var(--ok);
  }
  .link.syncing i {
    background: var(--warn);
    box-shadow: 0 0 6px var(--warn);
  }
  .link.connecting i {
    background: var(--err);
    box-shadow: 0 0 6px var(--err);
    animation: blink 0.8s steps(2) infinite;
  }
  .rev {
    color: #8b96a4;
    font-size: 10.5px;
    letter-spacing: 0;
  }
  .failover {
    height: 24px;
    line-height: 24px;
    padding: 0 8px;
    border-radius: 3px;
    background: var(--warn);
    color: #1a1200;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
  }
  .pending {
    font-size: 10.5px;
    color: var(--warn);
  }
  .replicas {
    display: flex;
    gap: 3px;
  }
  .replica {
    font-family: var(--font-num);
    font-size: 10.5px;
    padding: 0 5px;
    height: 20px;
    line-height: 18px;
    border-radius: 3px;
    border: 1px solid var(--ok);
    color: var(--ok);
  }
  .replica sup {
    font-size: 8px;
  }
  .replica.serving {
    background: var(--ok);
    color: #04140a;
  }
  .replica.down {
    border-color: var(--err);
    color: var(--err);
    background: transparent;
    text-decoration: line-through;
  }
  @keyframes blink {
    50% {
      opacity: 0.2;
    }
  }
</style>
