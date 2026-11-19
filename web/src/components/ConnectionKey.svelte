<script lang="ts">
      import { MediaQuery } from 'svelte/reactivity';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { TRANSMITTING, type LinkState } from '../lib/sync/machines';
  import { follow, type Placement } from '../lib/anchor';
  import { quorumOf } from '../lib/quorum';
  import { tip } from '../lib/tip';
  import { handleTag } from '../lib/net/handle';
  import { about, openAbout } from '../lib/about.svelte';

  /** In the status bar the details open upward; on a short screen the key sits in the header and they open downward. */
  let { where = 'bar' }: { where?: 'bar' | 'header' } = $props();
  const mixer = getMixer();
  let open = $state(false);

  const LABEL: Record<LinkState, string> = {
    Connecting: 'CONNECTING',
    Syncing: 'SYNC',
    Live: 'LIVE',
    Suspect: 'SUSPECT',
    FailingOver: 'FAILOVER',
    Resyncing: 'RESYNC',
    Ahead: 'AHEAD',
    Offline: 'OFFLINE',
  };
  /**
   * A short screen (App.svelte), and below 1200 px, where the header has
   * the least room: the words fit 8ch.
   */
  const short = new MediaQuery('max-height: 500px');
  const wide = new MediaQuery('min-width: 1200px');
  const phone = new MediaQuery('max-width: 700px');
  const BRIEF: Record<LinkState, string> = { ...LABEL, Connecting: 'CONN' };
  const brief = $derived(short.current || !wide.current);
  const status = $derived(mixer.status);
  const quorum = $derived(quorumOf(status.replicas, status.primaryReplica));
  /** Quorum says nothing while the link transmits no commands: the slot shows why writes are off instead. */
  const transmitting = $derived(TRANSMITTING.has(status.link));
  /** How much of the quorum's text the header has room for: the word, its initial, or once lost on a phone the word on two lines without the count. */
  const size = $derived(short.current || phone.current ? 'tight' : wide.current ? 'full' : 'brief');
  const quorumText = $derived.by(() => {
    if (quorum.commitable === null) return '';
    const count = `${quorum.commitableCount}/${quorum.total}`;
    if (quorum.commitable) return size === 'full' ? `QUORUM ${count}` : `Q ${count}`;
    return size === 'full' ? `NO QUORUM ${count}` : size === 'brief' ? `NO Q ${count}` : 'NO QUORUM';
  });
  const quorumTip = $derived.by(() => {
    const q = quorum;
    const down = status.replicas.filter((r) => !r.healthy).map((r) => r.id);
    const counts = `${q.commitableCount} of ${q.total} replicas can commit, ${q.needed} needed`;
    const leader = `Leader ${q.leader ?? 'unknown'}`;
    const serving = status.primaryReplica ? `${status.primaryReplica} serves this screen` : 'No replica serves this screen';
    const head = q.commitable ? `Quorum held: ${counts}.` : `No quorum: ${counts}. Changes are not committing; audio continues.`;
    const lock = mixer.lockReason !== null ? ` Every write control is disabled: ${mixer.lockReason}.` : '';
    return `${head} ${leader}. ${serving}.${down.length > 0 ? ` Down: ${down.join(', ')}.` : ''}${lock}`;
  });
  const role = (id: string) =>
    id === status.primaryReplica ? 'primary' : id === status.standbyReplica ? 'standby' : 'other';
  const use = (id: string) => (role(id) === 'primary' ? 'primary' : role(id) === 'standby' ? `standby (${status.standby})` : '-');
  const health = (r: { healthy: boolean; commitable: boolean }) => (!r.healthy ? 'down' : r.commitable ? 'can commit' : 'cannot commit');

  let key = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();
  let place = $state<Placement>({ left: 0, top: 0 });
  // Beside the link key, its left edge on the key's, moved in where that
  // runs off the viewport.
  $effect(() => {
    if (!open || !key || !panel) return;
    return follow(key, panel, where === 'bar' ? 'above' : 'below', 'start', (p) => (place = p));
  });
  function onkeydown(e: KeyboardEvent): void {
    // Escape in the About dialog closes the dialog alone, back to its key here.
    if (open && e.key === 'Escape' && !about.open) {
      open = false;
      key?.focus();
    }
  }
</script>

<svelte:window {onkeydown} />

<!-- Every slot has a fixed size and keeps it when its text changes or
     empties. The replica rows are in the connection details. -->
<div class="status" aria-label="Connection" data-testid="connection">
  <!-- The link lamp and the quorum, one group in the operator's
       sightline; revision, replicas and the meter source open from the
       lamp. -->
  <span class="conn" class:inbar={where === 'bar'} data-size={size}>
    <button
      bind:this={key}
      class="link-key"
      aria-expanded={open}
      aria-controls="connection-details"
      aria-label={`Connection ${LABEL[status.link]}, details`}
      onclick={() => (open = !open)}
    >
      <span
        class="link"
        data-state={status.link}
        role="status"
        aria-live="polite"
        data-testid="link"
        title={`Revision ${status.revision}; primary ${status.primaryReplica ?? '-'}, standby ${status.standbyReplica ?? '-'} (${status.standby})`}
      >
        <i></i><b>{(brief ? BRIEF : LABEL)[status.link]}</b>
      </span>
    </button>
    <!-- The lock is a node of its own, not the quorum one restyled: a link
         that stops transmitting moves no node in the slot. -->
    <span class="slot q">
      {#if !transmitting}
        <button
          class="quorum fixed lost"
          data-testid="quorum"
          data-quorum="locked"
          data-locked={mixer.lockReason}
          aria-expanded={open}
          aria-controls="connection-details"
          aria-label={`Every write control is disabled: ${mixer.lockReason}`}
          title={`Every write control is disabled: ${mixer.lockReason}`}
          onclick={() => (open = !open)}
        >LOCK</button>
      {:else if quorum.commitable !== null}
        <button
          class="quorum fixed"
          class:lost={!quorum.commitable}
          class:stack={size === 'tight' && quorum.commitable === false}
          class:degraded={quorum.commitable && quorum.commitableCount < quorum.total}
          data-testid="quorum"
          data-quorum={quorum.commitable ? (quorum.commitableCount < quorum.total ? 'degraded' : 'held') : 'lost'}
          data-commitable={quorum.commitable}
          data-locked={mixer.lockReason}
          aria-expanded={open}
          aria-controls="connection-details"
          aria-label={quorumTip}
          onclick={() => (open = !open)}
          use:tip={quorumTip}
        >{quorumText}</button>
      {/if}
    </span>
  </span>
  <div
    bind:this={panel}
    class="details"
    class:open
    id="connection-details"
    role="region"
    aria-label="Connection details"
    style:left="{place.left}px"
    style:top={place.top === undefined ? null : `${place.top}px`}
    style:bottom={place.bottom === undefined ? null : `${place.bottom}px`}
  >
    <div class="dhead"><span>Connection</span><b data-state={status.link}>{LABEL[status.link]}</b></div>
    <dl class="facts">
      <dt>Revision</dt><dd class="num">{status.revision}</dd>
      <dt>Primary</dt><dd class="num">{status.primaryReplica ?? '-'}</dd>
      <dt>Standby</dt><dd class="num">{status.standbyReplica ?? '-'} ({status.standby})</dd>
      <dt>Meters from</dt><dd class="num">{mixer.meterReplica ?? '-'}</dd>
      <dt>Quorum</dt>
      <dd data-testid="quorum-detail">
        {#if quorum.commitable === null}-{:else}{quorum.commitable ? 'held' : 'lost'}, {quorum.commitableCount} of {quorum.total} can commit, {quorum.needed} needed{/if}
      </dd>
      <dt>Leader</dt><dd class="num">{quorum.leader ?? 'unknown'}</dd>
      <dt>Session</dt><dd class="num" data-testid="session-detail">{handleTag(status.handle)}</dd>
      <dt>Endpoint</dt><dd class="num">{mixer.endpoint ?? '-'}</dd>
      <dt>faderz</dt><dd class="num" data-testid="build-faderz" title={`The faderz version ${status.primaryReplica ?? '-'} runs`}>{status.build?.faderz ?? '-'}</dd>
      <dt>samplez</dt><dd class="num" data-testid="build-samplez" title={`The samplez release ${status.primaryReplica ?? '-'} was built against`}>{status.build?.samplez ?? '-'}</dd>
    </dl>
    {#if status.build}
      <button class="whatsnew" data-testid="build-about" onclick={openAbout}>About / What's new</button>
    {/if}
    <table class="replicas" aria-label="Replicas" data-testid="replicas">
      <thead><tr><th scope="col">Replica</th><th scope="col">Role</th><th scope="col">State</th><th scope="col">This screen</th></tr></thead>
      <tbody>
        {#each status.replicas as r (r.id)}
          <tr
            class="replica"
            class:serving={role(r.id) === 'primary'}
            class:down={!r.healthy}
            class:nocommit={r.healthy && !r.commitable}
            data-replica={r.id}
            data-role={role(r.id)}
            data-healthy={r.healthy}
            data-commitable={r.commitable}
          >
            <td class="num"><i aria-hidden="true"></i>{r.id}</td>
            <td>{r.role}</td>
            <td>{health(r)}</td>
            <td>{use(r.id)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  /* Items of the header's status group (App.svelte), or of the status bar. */
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
  /* The link lamp and the quorum read as one key with two fields. */
  .conn {
    --h: var(--item-h, 24px);
    flex: none;
    display: flex;
    align-items: stretch;
    height: var(--h);
    border-radius: 3px;
    background: var(--well);
  }
  /* In the status bar the key is as tall as the engine lamp. */
  .conn.inbar {
    --h: 18px;
  }
  .link b {
    display: inline-block;
    width: 12ch;
    white-space: nowrap;
  }
  .link-key {
    all: unset;
    cursor: pointer;
    border-radius: 3px;
  }
  .link-key:focus-visible {
    outline: 2px solid var(--focus);
  }
  .link {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 8px;
    height: var(--h);
    border-radius: 3px;
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
  /* A socket still opening is waited for, not failed: amber, like a sync. */
  .link[data-state='Connecting'] i,
  .link[data-state='Syncing'] i,
  .link[data-state='Resyncing'] i,
  .link[data-state='Ahead'] i,
  .link[data-state='Suspect'] i {
    background: var(--warn);
    box-shadow: 0 0 6px var(--warn);
  }
  .link[data-state='FailingOver'] i,
  .link[data-state='Offline'] i {
    background: var(--err);
    box-shadow: 0 0 6px var(--err);
    animation: blink 0.8s steps(2) infinite;
  }
  /* One width for every quorum text of a size (`size`): QUORUM, NO
     QUORUM and LOCK, each with a count of up to one digit a side. */
  .slot.q {
    font-family: var(--font-num);
    font-size: 10.5px;
    width: calc(8ch + 12px);
    border-left: 1px solid var(--line-strong);
  }
  .conn[data-size='full'] .slot.q {
    width: calc(13ch + 12px);
  }
  .conn[data-size='tight'] .slot.q {
    width: calc(5ch + 8px);
  }
  .quorum {
    all: unset;
    display: block;
    box-sizing: border-box;
    width: 100%;
    height: var(--h, 24px);
    line-height: var(--h, 24px);
    text-align: center;
    border-radius: 0 3px 3px 0;
    color: var(--text-dim);
    cursor: pointer;
  }
  /* A lost quorum on a phone: the words on two lines in the 18 px key. */
  .quorum.stack {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 2px;
    white-space: normal;
    font-size: 8.5px;
    line-height: 9px;
    letter-spacing: 0;
  }
  .quorum:focus-visible {
    outline: 2px solid var(--focus);
  }
  /* Quorum held without every replica: amber, before it is lost. */
  .quorum.degraded {
    color: var(--warn);
    box-shadow: inset 0 0 0 1px var(--warn);
  }
  .quorum.lost {
    background: var(--err);
    color: #fff;
    font-weight: 700;
  }
  /* Opens under the link key over the view, placed by follow(); it moves
     nothing. */
  .details {
    display: none;
    position: fixed;
    z-index: 40;
    box-sizing: border-box;
    width: max-content;
    min-width: min(300px, calc(100vw - 16px));
    max-width: calc(100vw - 16px);
    padding: 8px 10px 10px;
    flex-direction: column;
    gap: 8px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
  }
  .details.open {
    display: flex;
  }
  .dhead {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--line);
    font-size: 10px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .dhead b {
    color: var(--text);
    font-weight: 600;
  }
  .facts {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    column-gap: 16px;
    row-gap: 3px;
    margin: 0;
  }
  .facts dt {
    color: var(--text-faint);
  }
  .facts dd {
    margin: 0;
    color: var(--text);
    text-align: right;
  }
  .whatsnew {
    appearance: none;
    align-self: flex-end;
    padding: 3px 8px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel-2);
    color: var(--text);
    font-size: 11px;
    cursor: pointer;
  }
  @media (pointer: coarse) {
    .whatsnew {
      padding: 12px 14px;
    }
  }
  .replicas {
    border-collapse: collapse;
    width: 100%;
  }
  .replicas th {
    padding: 0 8px 3px 0;
    color: var(--text-faint);
    font-weight: 400;
    font-size: 10px;
    letter-spacing: 0.06em;
    text-align: left;
    text-transform: uppercase;
    border-bottom: 1px solid var(--line);
  }
  .replicas td {
    padding: 3px 8px 0 0;
    color: var(--text);
  }
  .replicas th:last-child,
  .replicas td:last-child {
    padding-right: 0;
  }
  .replica td:first-child {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .replica i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
  }
  .replica.serving td {
    font-weight: 600;
  }
  .replica.nocommit i {
    background: var(--warn);
  }
  .replica.nocommit td:nth-child(3) {
    color: var(--warn);
  }
  .replica.down i {
    background: var(--err);
  }
  .replica.down td:nth-child(3) {
    color: var(--err);
  }
  @media (max-width: 600px) {
    .link {
      padding: 0 6px;
      font-size: 10px;
    }
  }
  /* The narrowest phones (App.svelte): room for a locked desk's key. */
  @media (max-width: 389px) {
    .link {
      padding: 0 3px;
    }
  }
  @keyframes blink {
    50% {
      opacity: 0.2;
    }
  }
  /* Below 1200 px the header holds the view tabs or menu and every key
     with less room: the link word fits 8ch (BRIEF). */
  @media (max-width: 1199px) {
    .link b {
      width: 8ch;
    }
  }
  /* Short screens: the key sits in the header (App.svelte) and the
     link word takes less of its one row, which the notice line needs. */
  @media (max-height: 500px) {
    .link b {
      width: 8ch;
    }
  }
  /* Fingers: the key keeps its look and its hit area grows to 44 px. */
  @media (pointer: coarse) {
    .conn.inbar {
      --h: 26px;
    }
    .conn.inbar .slot.q {
      min-width: 44px;
    }
    .link-key,
    .quorum {
      position: relative;
    }
    .link-key::after,
    .quorum::after {
      content: '';
      position: absolute;
      inset: -9px 0;
    }
  }
</style>
