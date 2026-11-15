<script lang="ts">
  // The master section of the mixer view: VCAs, buses and the monitor.
  // It draws its meters on its own canvas, so where it lies over the
  // strips (narrow screens) no strip meter shows through it.
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { MeterLayer, provideMeterLayer } from '../lib/meters/layer';
  import BusMaster from './BusMaster.svelte';
  import VcaMaster from './VcaMaster.svelte';
  import MonitorSection from './MonitorSection.svelte';

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
    min-height: 0;
    overflow-x: auto;
    border-left: 2px solid var(--line-strong);
    background: var(--panel);
  }
  .row {
    display: flex;
    flex: 1;
    min-height: 0;
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
      min-height: max(100%, 560px);
    }
  }
</style>
