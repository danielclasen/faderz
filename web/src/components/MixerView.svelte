<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import ChannelStrip from './ChannelStrip.svelte';
  import BusMaster from './BusMaster.svelte';
  import VcaMaster from './VcaMaster.svelte';
  import MonitorSection from './MonitorSection.svelte';

  interface Props {
    selected: string | null;
    onselect: (id: string) => void;
  }
  let { selected, onselect }: Props = $props();

  const STRIP_W = 86;
  const BANK = 8;
  const mixer = getMixer();
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
  const kindTag: Record<string, string> = { program: 'PGM', me: 'M&E', cleanFeed: 'CF' };

  function goto(start: number): void {
    scroller?.scrollTo({ left: start * STRIP_W, behavior: 'smooth' });
  }
</script>

<div class="mixer">
  <div class="left">
    <div class="bankbar">
      <span class="panel-title">Banks</span>
      <div class="banks" role="group" aria-label="Channel banks">
        {#each banks as b, i (b.start)}
          <button class="key" aria-pressed={visibleBank === i} onclick={() => goto(b.start)}>{b.start + 1}-{b.end}</button>
        {/each}
      </div>
      <span class="meters-from" title="Meters are measured by the replica serving this client">
        meters <b class="num">{mixer.meterReplica ?? '-'}</b>
      </span>
    </div>
    <div class="scroller" bind:this={scroller} onscroll={() => (scrollLeft = scroller?.scrollLeft ?? 0)}>
      <div class="surfaces" style:width={`${shape.channels.length * STRIP_W}px`} aria-hidden="true">
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
        {#each shape.channels as channel, index (channel.id)}
          <ChannelStrip {channel} {index} selected={selected === channel.id} {onselect} />
        {/each}
      </div>
    </div>
  </div>
  <div class="masters" aria-label="Masters">
    {#each shape.vcas as vca (vca.id)}
      <VcaMaster {vca} />
    {/each}
    {#each shape.buses as bus (bus.id)}
      <BusMaster group="buses" element={bus} tag={kindTag[bus.kind]} selected={selected === bus.id} {onselect} />
    {/each}
    <MonitorSection />
  </div>
</div>

<style>
  .mixer {
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
  .bankbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
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
  .meters-from {
    margin-left: auto;
    font-size: 10px;
    color: var(--text-faint);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .meters-from b {
    color: var(--text-dim);
  }
  .scroller {
    flex: 1;
    min-height: 0;
    overflow-x: auto;
    overflow-y: hidden;
    display: flex;
    flex-direction: column;
    scroll-snap-type: x proximity;
  }
  .surfaces {
    position: relative;
    height: 14px;
    flex: none;
  }
  .surface {
    position: absolute;
    top: 3px;
    height: 9px;
    margin-left: 2px;
    border: 1px solid var(--owned);
    border-bottom: none;
    border-radius: 3px 3px 0 0;
    font-size: 8.5px;
    line-height: 8px;
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
  .strips :global(.strip:nth-child(8n + 1)) {
    scroll-snap-align: start;
  }
  .masters {
    display: flex;
    min-height: 0;
    overflow-x: auto;
    border-left: 2px solid var(--line-strong);
  }
  /* Tablet: strips above masters at fixed heights, the view scrolls. */
  @media (max-width: 1100px) {
    .mixer {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: 560px 470px;
      overflow-y: auto;
    }
    .masters {
      border-left: none;
      border-top: 2px solid var(--line-strong);
    }
  }
</style>
