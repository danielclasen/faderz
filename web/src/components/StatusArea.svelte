<script lang="ts">
  import { fit } from '../lib/format';
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { TRANSMITTING, type LinkState } from '../lib/sync/machines';

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
    Ahead: 'AHEAD',
    Offline: 'OFFLINE',
  };
  const status = $derived(mixer.status);
  let chipsWidth = $state(0);
  let reserve = $state(0);
  $effect(() => {
    if (chipsWidth > reserve) reserve = chipsWidth;
  });
  const failover = $derived(
    status.failover && mixer.failoverAt !== null && now - mixer.failoverAt < 8000 ? status.failover : null,
  );
  const serving = $derived(status.replicas.find((r) => r.id === status.primaryReplica));
  const commitable = $derived(serving ? serving.commitable : null);
  /** Quorum says nothing while the link transmits no commands: the slot shows why writes are off instead. */
  const transmitting = $derived(TRANSMITTING.has(status.link));
  const role = (id: string) =>
    id === status.primaryReplica ? 'primary' : id === status.standbyReplica ? 'standby' : 'other';
  const describe = (r: { id: string; role: string; healthy: boolean; commitable: boolean }) => {
    const use = role(r.id);
    const serves = use === 'primary' ? ', serving this client' : use === 'standby' ? `, standby (${status.standby})` : '';
    return `${r.id}: ${r.role}, ${r.healthy ? 'healthy' : 'unhealthy'}, ${r.commitable ? 'can commit' : 'cannot commit'}${serves}`;
  };
</script>

<!-- Every slot has a fixed size and keeps it when its text changes or
     empties; the replica chips, whose count follows the cluster, come
     first, so in the right-aligned header they move nothing else. -->
<div class="status" aria-label="Connection" data-testid="status">
  <!-- The chips empty while the link reconnects: the box keeps the widest
       it has been, so the consoles before it never move. -->
  <div class="replicas" aria-label="Replicas" data-testid="replicas" bind:clientWidth={chipsWidth} style:min-width={`${reserve}px`}>
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
        {r.id}<sup class:off={r.role !== 'leader'}>L</sup><sub class:off={role(r.id) !== 'standby'}>SB</sub>
      </span>
    {/each}
  </div>
  <span class="slot q">
    <!-- The lock is a node of its own, not the quorum one restyled: a link
         that stops transmitting moves no node in the slot. -->
    {#if !transmitting}
      <span
        class="quorum lost"
        data-testid="quorum"
        data-locked={mixer.lockReason}
        title={`Every write control is disabled: ${mixer.lockReason}`}
      >LOCK</span>
    {:else if commitable !== null}
      <span
        class="quorum"
        class:lost={!commitable}
        data-testid="quorum"
        data-commitable={commitable}
        data-locked={mixer.lockReason}
        role={mixer.locked ? 'alert' : undefined}
        title={commitable
          ? 'The serving replica can commit'
          : mixer.locked
            ? `Every write control is disabled: ${mixer.lockReason}`
            : 'The serving replica cannot commit (no quorum)'}
      >{commitable ? 'Q' : 'NO Q'}</span>
    {/if}
  </span>
  <span class="slot pend">
    {#if status.outbox > 0}
      <span class="pending fixed" data-testid="pending" title="Commands waiting for confirmation">{fit(`${status.outbox} pending`, 11)}</span>
    {/if}
  </span>
  <span class="slot fo">
    {#if failover}
      <span class="failover num" role="status" data-testid="failover" title={`Failover after ${failover.reason}`}>
        <span class="fixed">{`FAILOVER ${failover.from ?? '-'}->${failover.to ?? '-'}`.padEnd(15)}</span>
      </span>
    {/if}
  </span>
  <div
    class="link"
    data-state={status.link}
    role="status"
    aria-live="polite"
    data-testid="link"
    title={`Revision ${status.revision}; primary ${status.primaryReplica ?? '-'}, standby ${status.standbyReplica ?? '-'} (${status.standby})`}
  >
    <i></i><b>{LABEL[status.link]}</b>
  </div>
  <span class="sr-only" data-testid="revision">rev {status.revision}</span>
</div>

<style>
  /* Items of the header's right group: see Consoles. */
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
  .slot.q {
    width: 5ch;
  }
  .slot.pend {
    width: 11ch;
  }
  /* Left-aligned, with its text padded at the end: the target replica
     appearing after the source moves nothing. */
  .slot.fo {
    width: 17ch;
    justify-content: flex-start;
  }
  .link b {
    display: inline-block;
    width: 12ch;
    white-space: nowrap;
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
  .link[data-state='Ahead'] i,
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
    flex: none;
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
  .replica {
    display: inline-flex;
    width: 7ch;
    justify-content: center;
    box-sizing: border-box;
    font-family: var(--font-num);
    font-size: 10.5px;
    padding: 0 5px;
    height: 20px;
    line-height: 18px;
    border-radius: 3px;
    border: 1px solid var(--ok);
    color: var(--ok);
  }
  /* Both marks keep their place, shown or not. */
  .replica .off {
    visibility: hidden;
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
  @media (max-width: 1500px) {
    .slot.fo,
    .replicas {
      display: none;
    }
  }
  @media (max-width: 700px) {
    .slot.pend {
      display: none;
    }
  }
</style>
