<script lang="ts">
  import type { Eq, Hpf } from '../lib/contract/types';
  import { eqResponse, logFrequencies } from '../lib/audio/eq';

  interface Props {
    eq: Eq;
    hpf?: Hpf | null;
    width?: number;
    height?: number;
    range?: number;
    grid?: boolean;
    /** As wide as its container: `width` then sets the drawing's units, stretched to fit. */
    fill?: boolean;
  }
  let { eq, hpf = null, width = 60, height = 20, range = 18, grid = false, fill = false }: Props = $props();

  const freqs = logFrequencies(64);
  const points = $derived.by(() => {
    const r = eqResponse(eq, hpf, freqs);
    return r
      .map((db, i) => {
        const x = (i / (freqs.length - 1)) * width;
        const y = height / 2 - (Math.max(-range, Math.min(range, db)) / range) * (height / 2 - 1);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  });
  const active = $derived(eq.enabled || (hpf?.enabled ?? false));
  const decades = [100, 1000, 10000].map((f) => (Math.log10(f / 20) / 3) * width);
</script>

<svg
  class="curve"
  class:active
  class:fill
  viewBox={`0 0 ${width} ${height}`}
  width={fill ? '100%' : width}
  {height}
  preserveAspectRatio={fill ? 'none' : undefined}
  aria-hidden="true"
>
  {#if grid}
    {#each decades as x (x)}<line x1={x} x2={x} y1="0" y2={height} class="grid" />{/each}
  {/if}
  <line x1="0" x2={width} y1={height / 2} y2={height / 2} class="zero" />
  <polyline {points} />
</svg>

<style>
  .curve {
    display: block;
    background: var(--well);
    border-radius: 2px;
  }
  /* Stretched sideways: the lines keep their width on screen. */
  .fill * {
    vector-effect: non-scaling-stroke;
  }
  .zero,
  .grid {
    stroke: var(--line-strong);
    stroke-width: 0.5;
  }
  polyline {
    fill: none;
    stroke: var(--text-faint);
    stroke-width: 1.2;
    stroke-linejoin: round;
  }
  .active polyline {
    stroke: var(--afl);
  }
</style>
