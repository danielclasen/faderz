<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { handleTag } from '../lib/net/handle';
  import { fit } from '../lib/format';
  import { tip } from '../lib/tip';
  const mixer = getMixer();
  const counters = $derived(Object.entries(mixer.counters).map(([name, value]) => [name.replace(/_total$/, ''), value] as const));
  const nonzero = $derived(counters.filter(([, v]) => v > 0).length);
  /** Every counter, for the tooltip and the accessible name: none is ever cut silently. */
  const allCounters = $derived(
    counters.length === 0
      ? 'No counters yet'
      : `Counters of replica ${mixer.countersReplica ?? '-'}: ${counters.map(([n, v]) => `${n} ${v}`).join(', ')}`,
  );
  let box: HTMLDivElement | undefined = $state();
  let list: HTMLDivElement | undefined = $state();
  /** The list does not fit its box: show "n counters, k nonzero" in its place. */
  let compact = $state(false);
  function measure(): void {
    if (box && list) compact = list.scrollWidth > box.clientWidth;
  }
  $effect(() => {
    if (!box) return;
    const ro = new ResizeObserver(measure);
    ro.observe(box);
    return () => ro.disconnect();
  });
  // A counter that appears lengthens the list; a new digit does not (fixed widths).
  $effect(() => {
    void counters.length;
    measure();
  });
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
  <!-- Counts grow without a bound: each is padded to a fixed width, so a
       new digit moves nothing. -->
  <span class="label">counters <span class="fixed">{fit(mixer.countersReplica ?? '-', 3)}</span></span>
  <!-- Counters arrive after load. The box has a width of its own, so
       its content moves nothing outside it. The list shows when it fits;
       otherwise a count takes its place, both drawn in the same box, and
       every counter is in the box's tooltip and name. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="counters" bind:this={box} role="note" tabindex="0" aria-label={allCounters} use:tip={allCounters} data-testid="counters">
    <div class="list" class:hidden={compact} bind:this={list} aria-hidden="true">
      {#each counters as [name, value] (name)}
        <span class="c" class:nz={value > 0}><span>{name}</span> <b class="num fixed" data-testid="counter">{fit(value > 999999 ? '>999k' : String(value), 6)}</b></span>
      {/each}
    </div>
    <span class="compact" class:shown={compact} aria-hidden="true"
      ><b class="num">{counters.length}</b> counters, <b class="num" class:nz={nonzero > 0}>{nonzero}</b> nonzero</span
    >
  </div>
  <!-- A fixed box with left-aligned text: the endpoint empties while
       reconnecting and comes back, and nothing moves. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <span
    class="right num"
    role="note"
    tabindex="0"
    data-testid="session"
    use:tip={`${handleTag(mixer.status.handle)} ${mixer.endpoint ?? ''}: session tag, as other clients name this screen, and primary endpoint`}
    ><span class="tag">{handleTag(mixer.status.handle)}</span> {mixer.endpoint ?? ''}</span
  >
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
  .counters {
    flex: 1 1 0;
    min-width: 0;
    position: relative;
    height: 20px;
    overflow: hidden;
    contain: layout paint style;
  }
  .counters:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }
  .list,
  .compact {
    position: absolute;
    inset: 0 auto 0 0;
    display: flex;
    align-items: center;
    gap: 12px;
    width: max-content;
  }
  .compact {
    gap: 0;
    white-space: pre;
    visibility: hidden;
  }
  .list.hidden {
    visibility: hidden;
  }
  .compact b {
    color: var(--text-dim);
    font-weight: 400;
  }
  .compact b.nz {
    color: var(--warn);
  }
  .right {
    flex: 0 0 min(34ch, 40%);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    contain: layout paint style;
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
  /* Phones: the replica label goes (the counters' tooltip names it) and
     the session box keeps the tag (its tooltip has the endpoint), so
     the counters keep room for their count. */
  @media (max-width: 700px) {
    .bar {
      gap: 8px;
    }
    .label {
      display: none;
    }
    .notice-line {
      flex-basis: 40%;
    }
    .right {
      flex-basis: 8ch;
    }
  }
</style>
