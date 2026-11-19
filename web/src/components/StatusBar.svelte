<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { handleTag } from '../lib/net/handle';
  import NoticeLine from './NoticeLine.svelte';
  import ConnectionKey from './ConnectionKey.svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { CounterRates, WARN_WINDOW_MS, engineLamp } from '../lib/counters';
  import { follow, type Placement } from '../lib/anchor';
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

  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();
  let place = $state<Placement>({ left: 0, bottom: 28 });
  // Above the lamp, centred on it, moved in where that runs off the
  // viewport; its width follows the counters' names and values.
  $effect(() => {
    if (!open || !button || !panel) return;
    return follow(button, panel, 'above', 'centre', (p) => (place = p));
  });

  // While the commit banner holds the notice line, notices wait unseen:
  // they are timed only from when the line shows them again.
  $effect(() => {
    if (mixer.notCommitting) return;
    const timers = mixer.notices.map((n) => setTimeout(() => mixer.dismiss(n.id), n.kind === 'error' || n.kind === 'request' ? 8000 : 4000));
    return () => timers.forEach(clearTimeout);
  });
</script>

<footer class="bar">
  <!-- The engine lamp, the connection key and the session and endpoint
       first, at the left edge, where they stay put; the notices follow. -->
  <div class="engine-wrap">
    <button
      bind:this={button}
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
      <div
        bind:this={panel}
        class="counters"
        id="engine-counters"
        role="region"
        aria-label="Engine counters"
        style:left="{place.left}px"
        style:bottom="{place.bottom ?? 0}px"
      >
        <div class="head">Counters from {mixer.countersReplica ?? '-'}, totals</div>
        {#each counters as [n, value] (n)}
          <div class="c" class:nz={warnings.includes(n)}><span>{name(n)}</span><b class="num">{value}</b></div>
        {/each}
      </div>
    {/if}
  </div>
  <!-- The link lamp, the quorum and the connection details, right of the engine. -->
  {#if !short.current}<ConnectionKey where="bar" />{/if}
  <!-- A fixed box with left-aligned text: the endpoint empties while
       reconnecting and comes back, and nothing moves. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <span
    class="session num"
    role="note"
    tabindex="0"
    data-testid="session"
    use:tip={`${handleTag(mixer.status.handle)} ${mixer.endpoint ?? ''}: session tag, as other clients name this screen, and primary endpoint`}
    ><span class="tag">{handleTag(mixer.status.handle)}</span> {mixer.endpoint ?? ''}</span
  >
  <!-- Notices take this fixed line, never an overlay on the controls. -->
  <div class="notices">{#if !short.current}<NoticeLine />{/if}</div>
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
  /* Fingers: the keys keep their look; the bar is as tall as a 44 px
     hit area, which the engine lamp and the connection keys reach
     beyond their borders. */
  @media (pointer: coarse) {
    .bar {
      height: 44px;
    }
    .bar .engine {
      height: 26px;
      position: relative;
    }
    .bar .engine::after {
      content: '';
      position: absolute;
      inset: -9px 0;
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
  /* Opens upward over the view, outside the bar's clipping; it moves
     nothing. Fixed, so placed over its button by follow(). */
  .counters {
    position: fixed;
    z-index: 40;
    box-sizing: border-box;
    min-width: min(240px, calc(100vw - 16px));
    max-width: calc(100vw - 16px);
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
  .c span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .c b {
    color: var(--text);
    font-weight: 400;
  }
  .c.nz b {
    color: var(--warn);
  }
  .session {
    flex: 0 0 min(34ch, 30%);
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
     so the notices keep their room. */
  @media (max-width: 700px) {
    .bar {
      gap: 8px;
      /* The dev mock key, which floats over the bar's right end, sets it. */
      padding-right: calc(10px + var(--mock-reserve, 0px));
    }
    .session {
      flex: 0 0 8ch;
      min-width: 0;
    }
    .notices {
      flex: 0 1 36%;
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
