<script lang="ts">
  // The width of a channel or an output, as the channel strip's head
  // shows it (layoutBadge): ST, 5.1 or 7.1, nothing for mono. Wherever a
  // bus is named beside a channel's or output's width, this says it.
  import type { Layout } from '../lib/contract/types';
  import { layoutBadge } from '../lib/format';

  let { layout }: { layout: Layout } = $props();
  const text = $derived(layoutBadge(layout));
</script>

{#if text}<span class="badge" data-layout={layout}>{text}</span>{/if}

<style>
  .badge {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0;
    text-transform: none;
    color: #0b0d10;
    background: #8b96a4;
    border-radius: 2px;
    padding: 0 2px;
    line-height: 13px;
  }
</style>
