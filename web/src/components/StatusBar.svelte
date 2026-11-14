<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { sessionTag } from '../lib/sync/session';
  const mixer = getMixer();
  const counters = $derived(Object.entries(mixer.counters));
  const latest = $derived(mixer.notices.at(-1) ?? null);
  const others = $derived(mixer.notices.length - 1);

  $effect(() => {
    const timers = mixer.notices.map((n) => setTimeout(() => mixer.dismiss(n.id), n.kind === 'error' ? 8000 : 4000));
    return () => timers.forEach(clearTimeout);
  });
</script>

<footer class="bar">
  <!-- Notices take this fixed line, never an overlay on the controls. -->
  <div class="notice-line" role="log" aria-live="polite" aria-label="Notices">
    {#if latest}
      <div class="notice {latest.kind}" data-testid="notice" title={latest.text}>
        <span class="text">{latest.text}</span>
        <span class="count num" class:shown={latest.count > 1}>x{latest.count}</span>
        <span class="more num" class:shown={others > 0}>+{others}</span>
        <button onclick={() => mixer.dismiss(latest.id)} aria-label="Dismiss notice">x</button>
      </div>
    {/if}
  </div>
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
  .notice-line {
    flex: 0 0 min(44%, 560px);
    height: 20px;
    min-width: 0;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 20px;
    padding: 0 6px;
    border-radius: 3px;
    border-left: 3px solid var(--afl);
    background: var(--panel-2);
    color: var(--text);
    font-size: 11.5px;
  }
  .notice.conflict {
    border-left-color: var(--owned);
  }
  .notice.error {
    border-left-color: var(--err);
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .count,
  .more {
    visibility: hidden;
    width: 4ch;
    color: var(--text-dim);
  }
  .shown {
    visibility: visible;
  }
  .notice button {
    all: unset;
    cursor: pointer;
    color: var(--text-dim);
    padding: 0 4px;
  }
</style>
