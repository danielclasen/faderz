<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import type { MeterGroup, MeterReading } from '../lib/meters/store';

  interface Props {
    group: MeterGroup;
    id: string;
    target?: number;
    onreset?: () => void;
  }
  let { group, id, target = -23, onreset }: Props = $props();
  const mixer = getMixer();
  let reading = $state<MeterReading | undefined>();

  onMount(() => {
    const t = setInterval(() => (reading = mixer.meters.get(group, id)), 250);
    return () => clearInterval(t);
  });

  const fmt = (v: number | null | undefined) => (v === null || v === undefined ? '--.-' : v <= -100 ? '-inf' : v.toFixed(1));
  const l = $derived(reading?.loudness);
  const tp = $derived(reading?.truePeak ? Math.max(...reading.truePeak) : null);
  const dev = $derived(l?.integrated == null ? null : l.integrated - target);
</script>

<div class="loud" aria-label="Loudness">
  <div class="cell"><span>M</span><b class="num">{fmt(l?.momentary)}</b></div>
  <div class="cell"><span>S</span><b class="num">{fmt(l?.shortTerm)}</b></div>
  <div class="cell int" class:ok={dev !== null && Math.abs(dev) <= 1} class:off={dev !== null && Math.abs(dev) > 1} class:invalid={l && !l.integratedValid}>
    <span>I</span><b class="num" data-testid={`loudness-i-${id}`}>{fmt(l?.integrated)}</b>
  </div>
  <div class="cell"><span>LRA</span><b class="num">{fmt(l?.lra)}</b></div>
  <div class="cell" class:hot={tp !== null && tp > -1}><span>TP</span><b class="num">{fmt(tp)}</b></div>
  {#if onreset}
    <button class="key reset" onclick={onreset} title="Reset integrated loudness and LRA">RST</button>
  {/if}
</div>

<style>
  .loud {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px;
    font-size: 10px;
  }
  .cell {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 3px;
    background: var(--well);
    border-radius: 2px;
    padding: 1px 4px;
    color: #8b96a4;
  }
  .cell b {
    color: #e6edf3;
    font-weight: 400;
    font-size: 10.5px;
  }
  .int.ok b {
    color: var(--meter-low);
  }
  .int.off b {
    color: var(--meter-mid);
  }
  .int.invalid b {
    text-decoration: line-through;
  }
  .hot b {
    color: var(--meter-high);
  }
  .reset {
    height: 17px;
    font-size: 9px;
  }
</style>
