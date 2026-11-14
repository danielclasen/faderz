<script lang="ts">
  import { meterLayer } from '../lib/meters/layer';
  import type { MeterGroup } from '../lib/meters/store';

  interface Props {
    group: MeterGroup;
    id: string;
    legs: number;
    scale?: boolean;
    label?: string;
  }
  let { group, id, legs, scale = false, label = 'meter' }: Props = $props();

  // The view's meter layer draws into this element's box.
  const layer = meterLayer();
  let el: HTMLDivElement | undefined = $state();
  $effect(() => layer.register(el!, { group, id, legs, scale }));
</script>

<div class="meter" role="img" aria-label={label} data-meter={`${group}/${id}`} bind:this={el}></div>

<style>
  .meter {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    border-radius: 2px;
  }
</style>
