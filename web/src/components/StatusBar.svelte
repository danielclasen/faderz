<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { handleTag } from '../lib/net/handle';
  import NoticeLine from './NoticeLine.svelte';
  import ConnectionKey from './ConnectionKey.svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { COUNTER_NAMES, CounterRates, INPUT_COUNTERS, WARN_WINDOW_MS, counterInfo, engineLamp, inputReasons, inputsBehind, type Assessment } from '../lib/counters';
  import { follow, type Placement } from '../lib/anchor';
  const mixer = getMixer();
  /** Same query as the header's: on a short screen the notices show there. */
  const short = new MediaQuery('max-height: 500px');
  const rates = new CounterRates();
  let seen = $state.raw<Assessment>({ rises: {}, warnings: [], inputs: [] });
  let open = $state(false);
  /** The counter whose explanation shows, if any. */
  let help = $state<string | null>(null);
  // A fault counter's rise within the window, not its total, lights the
  // lamp, and an input at fault names itself; checked once a second.
  onMount(() => {
    const t = setInterval(() => {
      const next = rates.update(mixer.counters, Date.now(), mixer.countersReplica, mixer.counterInputs);
      if (JSON.stringify(next) !== JSON.stringify(seen)) seen = next;
    }, 1000);
    return () => clearInterval(t);
  });
  const seconds = WARN_WINDOW_MS / 1000;
  /** A channel as the strips name it, with its id where that differs. */
  function channel(id: string): string {
    const name = mixer.shape?.channels.find((c) => c.id === id)?.name;
    return name && name !== id ? `${name} (${id})` : id;
  }
  const stripName = (id: string) => mixer.shape?.channels.find((c) => c.id === id)?.name ?? id;
  /** The counters shown, in the contract's order, any it does not name after them; faults first. */
  const rows = $derived.by(() => {
    const names = [...COUNTER_NAMES.filter((n) => n in mixer.counters), ...Object.keys(mixer.counters).filter((n) => !COUNTER_NAMES.includes(n))];
    const all = names.map((n) => ({ name: n, info: counterInfo(n), value: mixer.counters[n]! }));
    return { faults: all.filter((r) => r.info.fault), other: all.filter((r) => !r.info.fault) };
  });
  /** What is wrong now, one line each: the inputs at fault, then the other fault counters that rose. */
  const faults = $derived.by(() => {
    return [
      ...seen.inputs.map((f) => ({ key: `input-${f.channel}`, input: f.channel, what: `Input ${channel(f.channel)}`, why: inputReasons(f).join('; ') })),
      ...seen.warnings
        .filter((n) => inputsBehind(n, seen.inputs).length === 0)
        .map((n) => ({ key: n, input: null, what: counterInfo(n).label, why: `${seen.rises[n]} in the last ${seconds} s` })),
    ];
  });
  const lampTip = $derived(
    faults.length > 0 ? faults.map((f) => `${f.what}: ${f.why}.`).join(' ') : `No engine fault in the last ${seconds} s.`,
  );

  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();
  let place = $state<Placement>({ left: 0, bottom: 28 });
  // Above the lamp, centred on it, moved in where that runs off the
  // viewport.
  $effect(() => {
    if (!open || !button || !panel) return;
    return follow(button, panel, 'above', 'centre', (p) => (place = p));
  });

  /** Escape closes an open explanation and returns to its button; with none open it does nothing here. */
  function onkeydown(e: KeyboardEvent): void {
    if (!open || help === null || e.key !== 'Escape') return;
    e.preventDefault();
    const name = help;
    help = null;
    panel?.querySelector<HTMLButtonElement>(`[aria-controls="help-${name}"]`)?.focus();
  }

  // While the commit banner holds the notice line, notices wait unseen:
  // they are timed only from when the line shows them again.
  $effect(() => {
    if (mixer.notCommitting) return;
    const timers = mixer.notices.map((n) => setTimeout(() => mixer.dismiss(n.id), n.kind === 'error' || n.kind === 'request' ? 8000 : 4000));
    return () => timers.forEach(clearTimeout);
  });
</script>

{#snippet row(r: { name: string; info: ReturnType<typeof counterInfo>; value: number })}
  {@const rise = seen.rises[r.name] ?? 0}
  {@const warn = seen.warnings.includes(r.name)}
  <div class="c" class:nz={warn} data-counter={r.name}>
    <!-- The label and its (i) are one button: the whole name is the target. -->
    <button
      class="what"
      aria-expanded={help === r.name}
      aria-controls="help-{r.name}"
      aria-label="What {r.info.label} means"
      onclick={() => (help = help === r.name ? null : r.name)}
      ><span class="label">{r.info.label}</span><span class="info" aria-hidden="true">i</span></button
    >
    <span class="rise num">{#if rise > 0}<span class="sr-only">last {seconds} s </span>+{rise}{/if}</span>
    <b class="num"><span class="sr-only">total </span>{r.value}</b>
  </div>
  {#if help === r.name}
    <p class="help" id="help-{r.name}" data-testid="counter-help">
      {r.info.help}
      {#if warn && inputsBehind(r.name, seen.inputs).length > 0}From {inputsBehind(r.name, seen.inputs).map((f) => channel(f.channel)).join(', ')}.{/if}
      <span class="code">{r.name}</span>
    </p>
  {/if}
{/snippet}

<svelte:window {onkeydown} />

<footer class="bar">
  <!-- The engine lamp, the connection key and the session and endpoint
       first, at the left edge, where they stay put; the notices follow. -->
  <div class="engine-wrap">
    <button
      bind:this={button}
      class="engine"
      class:warn={faults.length > 0}
      aria-expanded={open}
      aria-controls="engine-counters"
      onclick={() => (open = !open)}
      use:tip={lampTip}
      data-testid="engine"
    ><i></i><span class="slot"><span class="fixed lamp">{engineLamp(seen.warnings.length, seen.inputs.map((f) => stripName(f.channel)))}</span></span></button>
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
        <div class="head">Engine of {mixer.countersReplica ?? '-'}, totals since it started</div>
        {#if faults.length > 0}
          <ul class="now" aria-label="Faults now" data-testid="engine-faults">
            {#each faults as f (f.key)}
              <li data-input={f.input}><b>{f.what}</b>: {f.why}.</li>
            {/each}
          </ul>
        {/if}
        <div class="cols" aria-hidden="true"><span>last {seconds} s</span><span>total</span></div>
        <div class="group">Faults: a rise is a problem now</div>
        {#each rows.faults as r (r.name)}{@render row(r)}{/each}
        <div class="group">Other: expected at times</div>
        {#each rows.other as r (r.name)}{@render row(r)}{/each}
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
    aria-label={`Session ${handleTag(mixer.status.handle)} ${mixer.endpoint ?? ''}`}
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
  /* The lamp's text changes with every fault that comes and goes. A
     flex item is never a relayout boundary, so the text is a block child
     of the lamp's one width with size and layout containment: a change
     lays out that box, not the page. */
  .engine .slot {
    display: block;
    font-family: var(--font-num);
  }
  .engine .lamp {
    display: block;
    width: 20ch;
    height: 14px;
    line-height: 14px;
    contain: size layout style;
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
     nothing. Fixed, so placed over its button by follow(). One width,
     so an explanation opening moves no column. */
  .counters {
    position: fixed;
    z-index: 40;
    box-sizing: border-box;
    width: min(340px, calc(100vw - 16px));
    max-height: calc(100vh - 48px);
    overflow-y: auto;
    padding: 6px 8px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    box-shadow: 0 -6px 18px rgb(0 0 0 / 0.4);
    color: var(--text-dim);
    white-space: normal;
  }
  .counters .head,
  .counters .group {
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 10px;
  }
  .counters .head {
    margin-bottom: 4px;
  }
  .counters .group {
    margin: 6px 0 2px;
    padding-top: 4px;
    border-top: 1px solid var(--line);
    color: var(--text-faint);
  }
  .now {
    margin: 0 0 4px;
    padding: 4px 6px;
    list-style: none;
    border-left: 2px solid var(--warn);
    background: var(--panel-2);
    color: var(--text);
    line-height: 15px;
  }
  .now b {
    color: var(--warn);
    font-weight: 600;
  }
  .c {
    display: flex;
    align-items: center;
    gap: 6px;
    line-height: 18px;
    white-space: nowrap;
  }
  .c .what {
    all: unset;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    cursor: pointer;
  }
  .c .what:focus-visible {
    outline: 2px solid var(--focus);
  }
  .c .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .c .info {
    flex: none;
    box-sizing: border-box;
    width: 14px;
    height: 14px;
    border: 1px solid var(--line-strong);
    border-radius: 50%;
    font-size: 9px;
    font-style: italic;
    font-weight: 600;
    line-height: 12px;
    text-align: center;
    color: var(--text-faint);
  }
  .c .what:hover .info,
  .c .what[aria-expanded='true'] .info {
    color: var(--text);
    border-color: var(--text-dim);
  }
  /* The rise and the total in columns the header names. */
  .c .rise,
  .cols span:first-child {
    width: 9ch;
    margin-left: auto;
    text-align: right;
  }
  .c .rise {
    color: var(--warn);
  }
  .cols {
    display: flex;
    gap: 6px;
    font-size: 9.5px;
    color: var(--text-faint);
  }
  .cols span:last-child {
    min-width: 6ch;
    text-align: right;
  }
  .c b {
    min-width: 6ch;
    text-align: right;
    color: var(--text);
    font-weight: 400;
  }
  .c.nz .label,
  .c.nz b {
    color: var(--warn);
  }
  .help {
    margin: 2px 0 6px 0;
    padding: 4px 6px;
    border-radius: 3px;
    background: var(--panel-2);
    color: var(--text);
    line-height: 15px;
  }
  .help .code {
    display: block;
    margin-top: 2px;
    font-family: var(--font-num);
    font-size: 9.5px;
    color: var(--text-faint);
  }
  /* Fingers: each name a 44 px target; the list scrolls in the panel. */
  @media (pointer: coarse) {
    .c .what {
      min-height: 44px;
      min-width: 44px;
    }
    .c .info {
      width: 20px;
      height: 20px;
      line-height: 18px;
      font-size: 11px;
    }
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
    /* While the commit banner holds the notice line it takes a row of its
       own above the bar, the whole width, so its words stay readable. */
    .bar:has(:global([data-testid='commit-banner'])) {
      flex-wrap: wrap;
      height: auto;
      padding-top: 3px;
      padding-bottom: 3px;
      row-gap: 3px;
    }
    .bar:has(:global([data-testid='commit-banner'])) .notices {
      order: -1;
      flex: 1 0 100%;
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
