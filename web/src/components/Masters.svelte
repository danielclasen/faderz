<script lang="ts">
  // The master section of the mixer view: the duck groups, VCAs, buses
  // and the monitor. It draws its meters on its own canvas, so where it
  // lies over the strips (narrow screens) no strip meter shows through it.
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { MeterLayer, provideMeterLayer } from '../lib/meters/layer';
  import BusMaster from './BusMaster.svelte';
  import VcaMaster from './VcaMaster.svelte';
  import MonitorSection from './MonitorSection.svelte';
  import DuckTile from './DuckTile.svelte';

  interface Props {
    id: string;
    selected: string | null;
    onselect: (id: string) => void;
    /** Shown as an overlay; where the masters sit beside the strips this has no effect. */
    open: boolean;
  }
  let { id, selected, onselect, open }: Props = $props();

  const mixer = getMixer();
  const meters = provideMeterLayer(new MeterLayer(mixer.meters));
  let root: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  onMount(() => {
    meters.attach(root!, canvas!);
    return () => meters.destroy();
  });
  $effect(() => meters.setZones(mixer.state!.metering.zones));
  const shape = $derived(mixer.shape!);
  const kindTag: Record<string, string> = { program: 'PGM', me: 'M&E', cleanFeed: 'CF' };
</script>

<div class="masters" class:open {id} tabindex="-1" role="region" aria-label="Masters" data-testid="masters" bind:this={root}>
  <!-- A band of fixed tiles over the masters: first in view where the
       masters lie over the strips, and taking height, never width, beside them. -->
  {#if (shape.ducks ?? []).length > 0}
    <div class="ducks" role="group" aria-label="Duck groups">
      {#each shape.ducks ?? [] as duck, i (duck.id)}
        <DuckTile {duck} shortcut={i + 1} />
      {/each}
    </div>
  {/if}
  <div class="row">
    {#each shape.vcas as vca (vca.id)}
      <VcaMaster {vca} />
    {/each}
    {#each shape.buses as bus (bus.id)}
      <BusMaster group="buses" element={bus} tag={kindTag[bus.kind]} selected={selected === bus.id} {onselect} />
    {/each}
    <MonitorSection />
  </div>
  <canvas class="meter-layer" bind:this={canvas} aria-hidden="true"></canvas>
</div>

<style>
  .masters {
    position: relative;
    isolation: isolate;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-x: auto;
    border-left: 2px solid var(--line-strong);
    background: var(--panel);
  }
  /* As wide as its masters, wider than the pane where they scroll. */
  .row {
    display: flex;
    flex: 1;
    min-height: 0;
    min-width: max-content;
  }
  /* Kept in view where the masters scroll sideways. As wide as the
     masters below it, whatever its tiles would take in one row: the tiles
     wrap, so the band never widens the masters and narrows the strips. */
  .ducks {
    contain: inline-size;
    position: sticky;
    left: 0;
    display: flex;
    flex-wrap: wrap;
    flex: none;
    gap: 1px;
    border-bottom: 1px solid var(--line-strong);
    background: var(--line);
  }
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
  /* Narrow screens: the strips take the whole width and the masters lie
     over their right end, below the bank bar, while MASTERS is lit. Opening or closing them
     moves nothing under them. A short screen scrolls them vertically. */
  @media (max-width: 1399px) {
    .masters {
      display: none;
      position: absolute;
      top: var(--bankbar-h);
      right: 0;
      bottom: 0;
      z-index: 10;
      max-width: 100%;
      overflow: auto;
      overscroll-behavior: contain;
      border-left: 2px solid var(--accent);
      box-shadow: -8px 0 24px rgb(0 0 0 / 0.45);
    }
    .masters.open {
      display: flex;
    }
    .row {
      flex: 1 0 auto;
      min-height: 560px;
    }
  }
</style>
