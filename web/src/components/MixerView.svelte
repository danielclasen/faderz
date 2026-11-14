<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { MeterLayer, provideMeterLayer } from '../lib/meters/layer';
  import ChannelStrip from './ChannelStrip.svelte';
  import Masters from './Masters.svelte';
  import { fit } from '../lib/format';
  import { tip } from '../lib/tip';

  interface Props {
    selected: string | null;
    onselect: (id: string, section?: string) => void;
  }
  let { selected, onselect }: Props = $props();

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
  /** Channels whose direct out latched a true-peak over. */
  const channelOvers = $derived(shape.channels.filter((c) => mixer.meters.overs.has(`channels/${c.id}`)));
  /** Narrow screens only: the masters lie over the strips while this is set. */
  let mastersOpen = $state(false);
  // A skip link to the masters or the monitor opens them where they are an overlay.
  onMount(() => {
    const show = () => (mastersOpen = true);
    document.addEventListener('faderz:show-masters', show);
    return () => document.removeEventListener('faderz:show-masters', show);
  });

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
    const next = Math.min(banks.length - 1, Math.max(0, visibleBank + (e.key === ']' ? 1 : -1)));
    goto(banks[next]!.start);
  }

  /** Jump: a strip number or the start of a channel name; the strip scrolls to the left edge. */
  let jumpText = $state('');
  let jumpMiss = $state(false);
  function jump(): void {
    const q = jumpText.trim().toLowerCase();
    if (!q) return;
    const n = Number(q);
    const index = Number.isInteger(n) && n >= 1 ? n - 1 : shape.channels.findIndex((c) => c.name.toLowerCase().startsWith(q));
    jumpMiss = index < 0 || index >= shape.channels.length;
    if (!jumpMiss) {
      goto(index);
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

  // FOLLOW: the strip area follows the first connected surface's bank.
  let follow = $state(false);
  const followed = $derived(surfaces[0]?.bankOffset ?? null);
  $effect(() => {
    if (follow && followed !== null) goto(followed);
  });
</script>

<svelte:window onkeydown={keydown} />

<div class="mixer" bind:this={root}>
  <div class="left">
    <div class="bankbar">
      <span class="panel-title">Banks</span>
      {#if spillVca}
        <button class="key spill" aria-pressed="true" onclick={() => (spill = null)} title="Show every channel again" data-testid="spill"
          >VCA {spillVca.name} SPILL - EXIT</button
        >
      {/if}
      <div class="banks" role="group" aria-label="Channel banks" class:hidden={spillVca !== undefined}>
        {#each banks as b, i (b.start)}
          <button class="key" aria-pressed={visibleBank === i} onclick={() => goto(b.start)}>{b.start + 1}-{b.end}</button>
        {/each}
      </div>
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
          ? `Clear the true-peak overs of ${channelOvers.length} channel direct outs: ${channelOvers.map((c) => c.name).join(', ')}`
          : 'No channel direct out went over the true-peak limit'}
        use:tip={channelOvers.length > 0 ? `Direct out above ${mixer.state!.metering.zones.truePeakMax} dBTP: ${channelOvers.map((c) => c.name).join(', ')}. Press to clear.` : ''}
        data-testid="channel-overs"><span class="fixed">TP{fit(String(channelOvers.length), 3)}</span></button
      >
      <button
        class="key fine"
        aria-pressed={mixer.fine}
        onclick={() => (mixer.fine = !mixer.fine)}
        title="Fine: faders, pans and parameters move at a tenth of their travel, as with Shift held">FINE</button
      >
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
          <ChannelStrip {channel} index={shape.channels.indexOf(channel)} selected={selected === channel.id} {onselect} />
        {/each}
      </div>
    </div>
  </div>
  <Masters id="mixer-masters" {selected} {onselect} open={mastersOpen} />
  <canvas class="meter-layer" bind:this={canvas} aria-hidden="true"></canvas>
</div>

<style>
  .meter-layer {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
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
  .hidden {
    display: none;
  }
  .surfaces.hidden {
    display: block;
    visibility: hidden;
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
  .jump.miss {
    border-color: var(--err);
  }
  @media (max-width: 700px) {
    .jump,
    .follow {
      display: none;
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
  /* Phones: the bank keys name themselves, and the meter replica, in the
     connection details too, gives its slot to the keys. */
  @media (max-width: 700px) {
    .meters-from {
      display: none;
    }
    .bankbar .panel-title {
      display: none;
    }
  }
</style>
