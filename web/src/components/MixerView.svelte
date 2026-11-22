<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { MeterLayer, provideMeterLayer } from '../lib/meters/layer';
  import ChannelStrip from './ChannelStrip.svelte';
  import Masters from './Masters.svelte';
  import { fit } from '../lib/format';
  import { tip } from '../lib/tip';
  import Detail from './Detail.svelte';
  import DuckKey from './DuckKey.svelte';

  interface Props {
    selected: string | null;
    /** Processing section to show first. */
    section?: string | null;
    onselect: (id: string, section?: string) => void;
    onclose: () => void;
  }
  let { selected, section = null, onselect, onclose }: Props = $props();

  const STRIP_W = 86;
  const BANK = 8;
  const mixer = getMixer();
  const meters = provideMeterLayer(new MeterLayer(mixer.meters));
  let root: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  onMount(() => {
    meters.attach(root!, canvas!);
    return () => meters.destroy();
  });
  $effect(() => meters.setZones(mixer.state!.metering.zones));
  $effect(() => meters.setLoudnessTarget(mixer.state!.metering.loudness.target));
  const shape = $derived(mixer.shape!);
  const banks = $derived(
    Array.from({ length: Math.ceil(shape.channels.length / BANK) }, (_, i) => ({
      start: i * BANK,
      end: Math.min(shape.channels.length, (i + 1) * BANK),
    })),
  );
  const surfaces = $derived(mixer.state!.consoles.filter((c) => c.connected && c.protocol !== 'osc'));
  let scroller: HTMLDivElement | undefined = $state();
  let scrollLeft = $state(0);
  const visibleBank = $derived(Math.floor((scrollLeft + STRIP_W / 2) / (STRIP_W * BANK)));
  /** Channels that latched a true-peak over. */
  const channelOvers = $derived(shape.channels.filter((c) => mixer.meters.overs.has(`channels/${c.id}`)));
  /** Narrow screens only: the masters lie over the strips while this is set. */
  let mastersOpen = $state(false);
  // A skip link to the masters or the monitor opens them where they are an overlay.
  onMount(() => {
    const show = () => (mastersOpen = true);
    document.addEventListener('faderz:show-masters', show);
    return () => document.removeEventListener('faderz:show-masters', show);
  });

  /** Scrolls to the strip at `start` of the strip area (of the shown strips during a spill). */
  function goto(start: number): void {
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
    scroller?.scrollTo({ left: start * STRIP_W, behavior: still ? 'instant' : 'smooth' });
  }

  // [ and ] step through the banks, as bank keys on a surface do; not
  // while typing.
  function keydown(e: KeyboardEvent): void {
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA')) return;
    if (e.key !== '[' && e.key !== ']') return;
    e.preventDefault();
    const last = Math.max(0, Math.ceil(shown.length / BANK) - 1);
    goto(Math.min(last, Math.max(0, visibleBank + (e.key === ']' ? 1 : -1))) * BANK);
  }

  /** Jump: a strip number or the start of a channel name; the strip scrolls to the left edge. */
  let jumpText = $state('');
  let jumpMiss = $state(false);
  function jump(): void {
    const q = jumpText.trim().toLowerCase();
    if (!q) return;
    const n = Number(q);
    const channel = Number.isInteger(n) && n >= 1 ? shape.channels[n - 1] : shape.channels.find((c) => c.name.toLowerCase().startsWith(q));
    // During a spill only its members are on the strips: any other misses.
    const at = channel ? shown.indexOf(channel) : -1;
    jumpMiss = at < 0;
    if (!jumpMiss) {
      goto(at);
      jumpText = '';
    }
  }

  // VCA spill: the strips show only one VCA's members until EXIT.
  let spill = $state<string | null>(null);
  const spillVca = $derived(spill ? shape.vcas.find((v) => v.id === spill) : undefined);
  const shown = $derived(
    spill ? shape.channels.filter((c) => mixer.state!.vcas[spill!]?.members.includes(c.id)) : shape.channels,
  );
  onMount(() => {
    const on = (e: Event) => {
      spill = (e as CustomEvent<string>).detail;
      mastersOpen = false;
      scroller?.scrollTo({ left: 0 });
    };
    document.addEventListener('faderz:spill', on);
    return () => document.removeEventListener('faderz:spill', on);
  });

  // Sends on faders: the channel faders set one aux's send levels until EXIT.
  let sof = $state('');
  const sofAux = $derived(sof ? shape.auxes.find((a) => a.id === sof) : undefined);

  // FOLLOW: the strip area follows the first connected surface's bank.
  let follow = $state(false);
  const followed = $derived(surfaces[0]?.bankOffset ?? null);
  $effect(() => {
    if (!follow || followed === null) return;
    // During a spill: the first member at or after the surface's bank.
    const at = shown.findIndex((c) => shape.channels.indexOf(c) >= followed);
    if (at >= 0) goto(at);
  });
</script>

<svelte:window onkeydown={keydown} />

<div class="mixer" bind:this={root}>
  <div class="left">
    <div class="bankbar">
      <span class="panel-title">Banks</span>
      <!-- Fixed slots: what they show changes, their size does not. -->
      <span class="sof-slot">
        {#if sofAux}
          <button class="key sof-exit" aria-pressed="true" onclick={() => (sof = '')} data-testid="sends-on-faders" title={`SOF ${sofAux.name} - EXIT: the faders set the sends to ${sofAux.name}; press to give them back their channel levels`}
            >SOF {sofAux.name} - EXIT</button
          >
        {:else}
          <select class="sof" aria-label="Sends on faders" value="" onchange={(e) => (sof = (e.currentTarget as HTMLSelectElement).value)}>
            <option value="">Sends on faders...</option>
            {#each shape.auxes as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
          </select>
        {/if}
      </span>
      <span class="banks-slot">
        <div class="banks" role="group" aria-label="Channel banks" class:gone={spillVca !== undefined}>
          {#each banks as b, i (b.start)}
            <button class="key" aria-pressed={visibleBank === i} onclick={() => goto(b.start)}>{b.start + 1}-{b.end}</button>
          {/each}
        </div>
        {#if spillVca}
          <button class="key spill" aria-pressed="true" onclick={() => (spill = null)} title={`VCA ${spillVca.name} - EXIT: only its members are shown; press to show every channel`} data-testid="spill"
            >VCA {spillVca.name} - EXIT</button
          >
        {/if}
      </span>
      <input
        class="jump"
        type="text"
        bind:value={jumpText}
        class:miss={jumpMiss}
        oninput={() => (jumpMiss = false)}
        onkeydown={(e) => e.key === 'Enter' && jump()}
        placeholder="Jump"
        aria-label="Jump to a channel: number or name, Enter"
        title="Type a strip number or the start of a channel name and press Enter"
      />
      <button
        class="key follow"
        aria-pressed={follow}
        disabled={surfaces.length === 0}
        onclick={() => (follow = !follow)}
        title={surfaces[0] ? `Follow the bank of ${surfaces[0].name}` : 'No control surface connected'}>FOLLOW</button
      >
      <button
        class="key overs"
        class:lit={channelOvers.length > 0}
        disabled={channelOvers.length === 0}
        onclick={() => mixer.meters.clearOvers('channels', shape.channels.map((c) => c.id))}
        aria-label={channelOvers.length > 0
          ? `Clear the true-peak overs of ${channelOvers.length} channels: ${channelOvers.map((c) => c.name).join(', ')}`
          : 'No channel went over the true-peak limit'}
        use:tip={channelOvers.length > 0 ? `Channel true peak above ${mixer.state!.metering.zones.truePeakMax} dBTP: ${channelOvers.map((c) => c.name).join(', ')}. Press to clear.` : ''}
        data-testid="channel-overs"><span class="fixed">TP{fit(String(channelOvers.length), 3)}</span></button
      >
      <button
        class="key fine"
        aria-pressed={mixer.fine}
        onclick={() => (mixer.fine = !mixer.fine)}
        title="Fine: faders, pans and parameters move at a tenth of their travel, as with Shift held">FINE</button
      >
      <!-- Where the masters lie over the strips, every duck group stays in
           sight and in reach with them closed: a DUCK key per group that
           says what the group does. -->
      {#if (shape.ducks ?? []).length > 0}
        <span class="duck-pills" role="group" aria-label="Duck groups in the bank bar">
          {#each shape.ducks ?? [] as d (d.id)}<DuckKey id={d.id} name={d.name} pill />{/each}
        </span>
      {/if}
      <button
        class="key masters-key"
        aria-pressed={mastersOpen}
        aria-controls="mixer-masters"
        onclick={() => (mastersOpen = !mastersOpen)}
        title="Show the masters over the strips">MASTERS</button
      >
      <!-- Shown only while the meters come from another replica than the state. -->
      <span
        class="meters-from"
        class:other={mixer.meterReplica !== null && mixer.meterReplica !== mixer.status.primaryReplica}
        title="Replica that measured the meters shown, not the one serving the state"
        data-testid="meter-replica">
        meters <b class="num">{mixer.meterReplica ?? '-'}</b>
      </span>
    </div>
    <div class="scroller" role="region" aria-label="Channel strips" bind:this={scroller} onscroll={() => (scrollLeft = scroller?.scrollLeft ?? 0)}>
      <div class="surfaces" class:hidden={spillVca !== undefined} style:width={`${shape.channels.length * STRIP_W}px`} aria-hidden="true">
        {#each surfaces as s (s.id)}
          <span
            class="surface"
            style:left={`${s.bankOffset * STRIP_W}px`}
            style:width={`${Math.min(s.strips, shape.channels.length - s.bankOffset) * STRIP_W - 4}px`}
            title={`${s.name} (${s.protocol.toUpperCase()}) controls these strips`}>{s.name}</span
          >
        {/each}
      </div>
      <div class="strips">
        {#each shown as channel (channel.id)}
          <ChannelStrip {channel} index={shape.channels.indexOf(channel)} selected={selected === channel.id} {onselect} sendsTo={sofAux ?? null} />
        {/each}
      </div>
    </div>
  </div>
  <!-- The processing panel lies over the right end of the strip area,
       never over the masters and the monitor. -->
  {#if selected}
    {#key selected}
      <Detail id={selected} {section} {onclose} onselect={(id) => onselect(id)} />
    {/key}
  {/if}
  <Masters id="mixer-masters" {selected} {onselect} open={mastersOpen} />
  <canvas class="meter-layer" bind:this={canvas} aria-hidden="true"></canvas>
</div>

<style>
  /* Anchored at the corner; the meter layer sizes it and moves it over
     the visible meters by a transform. */
  .meter-layer {
    position: absolute;
    top: 0;
    left: 0;
    width: 0;
    height: 0;
    pointer-events: none;
    z-index: 1;
  }
  /* The meter layer stacks above the strips and below anything outside
     the view, such as the processing panel. */
  .mixer {
    --bankbar-h: 31px;
    position: relative;
    isolation: isolate;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    height: 100%;
    min-height: 0;
  }
  .left {
    grid-column: 1;
    grid-row: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  /* Fixed height: the masters overlay starts right below it. */
  .bankbar {
    display: flex;
    align-items: center;
    gap: 8px;
    white-space: nowrap;
    height: var(--bankbar-h);
    box-sizing: border-box;
    padding: 0 8px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .banks {
    display: flex;
    gap: 3px;
  }
  .banks .key {
    font-family: var(--font-num);
    font-size: 10px;
  }
  .overs {
    flex: none;
    font-size: 10px;
    padding: 0 5px;
  }
  .overs.lit {
    background: var(--meter-high);
    border-color: var(--meter-high);
    color: #fff;
  }
  .meters-from {
    margin-left: auto;
    font-size: 10px;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .meters-from {
    visibility: hidden;
  }
  .meters-from.other {
    visibility: visible;
  }
  .meters-from b {
    display: inline-block;
    min-width: 4ch;
    color: var(--text-dim);
  }
  /* The scroller shows whole strips only: its width is a multiple of a
     strip, and every scroll ends on a strip edge. */
  .scroller {
    flex: 1;
    width: round(down, 100%, 86px);
    min-height: 0;
    overflow-x: auto;
    overflow-y: hidden;
    display: flex;
    flex-direction: column;
    scroll-snap-type: x mandatory;
  }
  .surfaces {
    position: relative;
    height: 17px;
    flex: none;
  }
  .surface {
    position: absolute;
    top: 3px;
    height: 13px;
    margin-left: 2px;
    border: 1px solid var(--owned);
    border-bottom: none;
    border-radius: 3px 3px 0 0;
    font-size: 10px;
    line-height: 12px;
    color: var(--owned);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    padding-left: 4px;
  }
  .strips {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .strips :global(.strip) {
    scroll-snap-align: start;
  }
  .masters-key {
    display: none;
  }
  .surfaces.hidden {
    display: block;
    visibility: hidden;
  }
  .sof-exit[aria-pressed='true'] {
    background: var(--meter-loud);
    border-color: var(--meter-loud);
    color: #0b0d10;
  }
  .sof-slot {
    flex: none;
    width: 19ch;
    display: flex;
  }
  .sof,
  .sof-exit {
    width: 100%;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sof {
    height: 22px;
    font-size: 11px;
  }
  .banks-slot {
    position: relative;
    flex: none;
  }
  .banks.gone {
    visibility: hidden;
  }
  .spill {
    position: absolute;
    inset: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @media (pointer: coarse) {
    .sof {
      height: 32px;
    }
  }
  @media (max-width: 700px) {
    .sof-slot {
      display: none;
    }
  }
  .spill[aria-pressed='true'] {
    background: var(--owned);
    border-color: var(--owned);
    color: #fff;
  }
  .jump {
    width: 9ch;
    height: 22px;
    font-size: 11px;
  }
  @media (pointer: coarse) {
    .jump {
      height: 32px;
    }
  }
  .jump.miss {
    border-color: var(--err);
  }
  @media (max-width: 700px) {
    .jump,
    .follow {
      display: none;
    }
  }
  .duck-pills {
    display: none;
  }
  @media (min-width: 1024px) and (max-width: 1399px) {
    .duck-pills {
      display: flex;
      gap: 3px;
      min-width: 0;
      overflow-x: auto;
      scrollbar-width: none;
    }
  }
  @media (max-width: 1399px) {
    .mixer {
      grid-template-columns: minmax(0, 1fr);
    }
    .masters-key {
      display: block;
      margin-left: auto;
    }
    .meters-from {
      margin-left: 0;
    }
  }
  /* The narrowest phones: tighter keys, so the bank keys, TP, FINE and
     MASTERS keep their full text within 320 px. */
  @media (max-width: 389px) {
    .bankbar {
      gap: 4px;
      padding: 0 4px;
    }
    .banks .key,
    .overs {
      padding: 0 3px;
    }
  }
  /* Below 760 px the meter replica, in the connection details too, gives
     its slot to the keys, which otherwise do not fit. */
  @media (max-width: 759px) {
    .meters-from {
      display: none;
    }
  }
  /* Phones: the bank keys name themselves. */
  @media (max-width: 700px) {
    .bankbar .panel-title {
      display: none;
    }
  }
</style>
