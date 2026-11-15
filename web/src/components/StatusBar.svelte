<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { handleTag } from '../lib/net/handle';
  import NoticeLine from './NoticeLine.svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { CounterRates, WARN_WINDOW_MS, engineLamp } from '../lib/counters';
  const mixer = getMixer();
  /** Same query as the header's: on a short screen the notices show there. */
  const short = new MediaQuery('max-height: 500px');
  const counters = $derived(Object.entries(mixer.counters));
  const rates = new CounterRates();
  let warnings = $state<string[]>([]);
  let open = $state(false);
  // A counter's growth, not its total, lights the lamp; checked once a second.
  onMount(() => {
    const t = setInterval(() => {
      const next = rates.update(mixer.counters, Date.now(), mixer.countersReplica);
      if (next.join() !== warnings.join()) warnings = next;
    }, 1000);
    return () => clearInterval(t);
  });
  const name = (n: string) => n.replace(/_total$/, '');

  $effect(() => {
    const timers = mixer.notices.map((n) => setTimeout(() => mixer.dismiss(n.id), n.kind === 'error' || n.kind === 'request' ? 8000 : 4000));
    return () => timers.forEach(clearTimeout);
  });
</script>

<footer class="bar">
  <!-- Notices take this fixed line, never an overlay on the controls. -->
  <div class="notices">{#if !short.current}<NoticeLine />{/if}</div>
  <div class="engine-wrap">
    <button
      class="engine"
      class:warn={warnings.length > 0}
      aria-expanded={open}
      aria-controls="engine-counters"
      onclick={() => (open = !open)}
      use:tip={warnings.length > 0
        ? `Growing in the last ${WARN_WINDOW_MS / 1000} s: ${warnings.map(name).join(', ')}`
        : `No engine counter grew in the last ${WARN_WINDOW_MS / 1000} s`}
      data-testid="engine"
    ><i></i><span class="fixed">{engineLamp(warnings.length)}</span></button>
    {#if open}
      <div class="counters" id="engine-counters" role="region" aria-label="Engine counters">
        <div class="head">Counters from {mixer.countersReplica ?? '-'}, totals</div>
        {#each counters as [n, value] (n)}
          <div class="c" class:nz={warnings.includes(n)}><span>{name(n)}</span><b class="num">{value}</b></div>
        {/each}
      </div>
    {/if}
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
  @media (pointer: coarse) {
    .bar {
      height: 32px;
    }
    .bar .engine {
      height: 26px;
    }
  }
  .engine-wrap {
    position: relative;
    flex: none;
  }
  .engine {
    all: unset;
    display: flex;
    align-items: center;
    gap: 5px;
    height: 18px;
    padding: 0 6px;
    border-radius: 3px;
    border: 1px solid var(--line-strong);
    color: var(--text-dim);
    font-size: 10.5px;
    cursor: pointer;
  }
  .engine:focus-visible {
    outline: 2px solid var(--focus);
  }
  .engine i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
  }
  .engine.warn {
    color: var(--warn);
    border-color: var(--warn);
  }
  .engine.warn i {
    background: var(--warn);
  }
  /* Opens upward over the view, outside the bar's clipping; it moves nothing. */
  .counters {
    position: fixed;
    left: 10px;
    bottom: 28px;
    z-index: 40;
    min-width: 240px;
    padding: 6px 8px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    box-shadow: 0 -6px 18px rgb(0 0 0 / 0.4);
    color: var(--text-dim);
  }
  .counters .head {
    margin-bottom: 4px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 10px;
  }
  .c {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    line-height: 16px;
  }
  .c b {
    color: var(--text);
    font-weight: 400;
  }
  .c.nz b {
    color: var(--warn);
  }
  .right {
    flex: 0 0 min(34ch, 40%);
    margin-left: auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    contain: layout paint style;
  }
  .notices {
    flex: 0 0 min(44%, 560px);
    min-width: 0;
  }
  /* Phones: the session box keeps the tag (its tooltip has the endpoint),
     so the notices keep their room; the engine lamp, sized for counts up
     to 99+, still leaves a free stretch before the session box. */
  @media (max-width: 700px) {
    .bar {
      gap: 8px;
    }
    .notices {
      flex-basis: 36%;
    }
    .right {
      flex-basis: 8ch;
    }
  }
  /* Short screens: the bar gives its row to the faders; notices show in
     the header instead. */
  @media (max-height: 500px) {
    .bar {
      display: none;
    }
  }
</style>
