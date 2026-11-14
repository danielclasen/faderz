<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import type { LinkState } from '../lib/sync/machines';

  const mixer = getMixer();
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 500);
    return () => clearInterval(t);
  });

  const LABEL: Record<LinkState, string> = {
    Connecting: 'RECONNECTING',
    Syncing: 'SYNC',
    Live: 'LIVE',
    Suspect: 'SUSPECT',
    FailingOver: 'FAILOVER',
    Resyncing: 'RESYNC',
    Offline: 'OFFLINE',
  };
  const status = $derived(mixer.status);
  const failover = $derived(
    status.failover && mixer.failoverAt !== null && now - mixer.failoverAt < 8000 ? status.failover : null,
  );
  const serving = $derived(status.replicas.find((r) => r.id === status.primaryReplica));
  const commitable = $derived(serving ? serving.commitable : null);
  const role = (id: string) =>
    id === status.primaryReplica ? 'primary' : id === status.standbyReplica ? 'standby' : 'other';
  const describe = (r: { id: string; role: string; healthy: boolean; commitable: boolean }) => {
    const use = role(r.id);
    const serves = use === 'primary' ? ', serving this client' : use === 'standby' ? `, standby (${status.standby})` : '';
    return `${r.id}: ${r.role}, ${r.healthy ? 'healthy' : 'unhealthy'}, ${r.commitable ? 'can commit' : 'cannot commit'}${serves}`;
  };
</script>

<div class="status" aria-label="Connection" data-testid="status">
  <div
    class="link"
    data-state={status.link}
    role="status"
    aria-live="polite"
    data-testid="link"
    title={`Primary ${status.primaryReplica ?? '-'}, standby ${status.standbyReplica ?? '-'} (${status.standby})`}
  >
    <i></i><b>{LABEL[status.link]}</b>
    <span class="num rev" title="Global state revision" data-testid="revision"><span>rev {status.revision}</span></span>
  </div>
  {#if failover}
    <div class="failover" role="status" data-testid="failover" title={`Failover after ${failover.reason}`}>
      FAILOVER {failover.from ?? '-'} -&gt; {failover.to ?? '-'}
    </div>
  {/if}
  {#if status.outbox > 0}
    <span class="pending num" data-testid="pending" title="Commands waiting for confirmation">{status.outbox} pending</span>
  {/if}
  <div class="replicas" aria-label="Replicas" data-testid="replicas">
    {#each status.replicas as r (r.id)}
      <span
        class="replica"
        class:serving={role(r.id) === 'primary'}
        class:standby={role(r.id) === 'standby'}
        class:ready={role(r.id) === 'standby' && status.standby === 'Ready'}
        class:down={!r.healthy}
        class:nocommit={r.healthy && !r.commitable}
        title={describe(r)}
        data-replica={r.id}
        data-role={role(r.id)}
        data-healthy={r.healthy}
        data-commitable={r.commitable}
      >
        {r.id}{#if r.role === 'leader'}<sup>L</sup>{/if}{#if role(r.id) === 'standby'}<sub>SB</sub>{/if}
      </span>
    {/each}
  </div>
  {#if commitable !== null}
    <span
      class="quorum"
      class:lost={!commitable}
      data-testid="quorum"
      data-commitable={commitable}
      role={mixer.locked ? 'alert' : undefined}
      title={commitable
        ? 'The serving replica can commit'
        : mixer.locked
          ? 'No quorum and no standby: every write control is disabled'
          : 'The serving replica cannot commit (no quorum)'}
    >{commitable ? 'Q' : 'NO Q'}</span>
  {/if}
</div>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
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
  .link[data-state='Syncing'] i,
  .link[data-state='Resyncing'] i,
  .link[data-state='Suspect'] i {
    background: var(--warn);
    box-shadow: 0 0 6px var(--warn);
  }
  .link[data-state='Connecting'] i,
  .link[data-state='FailingOver'] i,
  .link[data-state='Offline'] i {
    background: var(--err);
    box-shadow: 0 0 6px var(--err);
    animation: blink 0.8s steps(2) infinite;
  }
  .rev {
    display: inline-block;
    width: 11ch;
    color: #8b96a4;
    font-size: 10.5px;
    letter-spacing: 0;
  }
  /* The text changes with every update. A flex item is never a relayout
     boundary, so the text sits in a block child of fixed size with strict
     containment, which lays out nothing outside it. */
  .rev span {
    display: block;
    height: 14px;
    line-height: 14px;
    overflow: hidden;
    white-space: nowrap;
    contain: strict;
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
    white-space: nowrap;
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
  .replica sup,
  .replica sub {
    font-size: 8px;
  }
  .replica.serving {
    background: var(--ok);
    color: #04140a;
  }
  .replica.standby {
    border-style: dashed;
  }
  .replica.standby.ready {
    border-style: solid;
    box-shadow: inset 0 0 0 1px var(--ok);
  }
  .replica.nocommit {
    border-color: var(--warn);
    color: var(--warn);
  }
  .replica.serving.nocommit {
    background: var(--warn);
    color: #1a1200;
  }
  .replica.down {
    border-color: var(--err);
    color: var(--err);
    background: transparent;
    text-decoration: line-through;
  }
  .quorum {
    font-family: var(--font-num);
    font-size: 10px;
    padding: 0 4px;
    height: 20px;
    line-height: 20px;
    border-radius: 3px;
    color: var(--text-faint);
  }
  .quorum.lost {
    height: 24px;
    line-height: 24px;
    padding: 0 10px;
    font-size: 12px;
    letter-spacing: 0.08em;
    background: var(--err);
    color: #fff;
    font-weight: 700;
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--err) 40%, transparent);
  }
  @media (max-width: 600px) {
    .rev {
      display: none;
    }
    .link {
      padding: 0 6px;
      font-size: 10px;
    }
    .replica {
      padding: 0 3px;
      font-size: 10px;
    }
  }
  @keyframes blink {
    50% {
      opacity: 0.2;
    }
  }
</style>
