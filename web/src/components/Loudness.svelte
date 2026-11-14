<script lang="ts">
  import { fit } from '../lib/format';
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import type { MeterGroup, MeterReading } from '../lib/meters/store';

  /** Readout period, ms. */
  const READOUT_MS = 100;

  interface Props {
    group: MeterGroup;
    id: string;
    /** Stereo elements show their phase correlation. */
    stereo?: boolean;
    onreset?: () => void;
  }
  let { group, id, stereo = false, onreset }: Props = $props();
  const mixer = getMixer();
  let reading = $state<MeterReading | undefined>();
  /** Highest true peak of every frame since the previous readout tick, dBTP. */
  let tp = $state<number | null>(null);
  const tpLegs: number[] = [];

  // 10 readouts per second, the rate EBU Tech 3341 asks of a live meter.
  // The true peak is the maximum over every frame in between, so a short
  // over between two ticks is still shown.
  onMount(() => {
    let since = -Infinity;
    const t = setInterval(() => {
      const meters = mixer.meters;
      reading = meters.get(group, id);
      tp = meters.maxSince(group, id, 'truePeak', since, tpLegs) ? Math.max(...tpLegs) : reading ? tp : null;
      since = meters.receivedAt;
    }, READOUT_MS);
    return () => clearInterval(t);
  });
  const over = $derived(mixer.meters.overs.has(`${group}/${id}`));
  function reset(): void {
    if (mixer.locked) return;
    mixer.meters.clearOvers(group, [id]);
    onreset?.();
  }

  /** One decimal place at most (EBU Tech 3341); every value fits 5 characters. */
  const fmt = (v: number | null | undefined) => fit(v === null || v === undefined ? '--.-' : v <= -100 ? '-inf' : v.toFixed(1), 5);
  const signed = (v: number | null | undefined) => fit(v === null || v === undefined ? '--.--' : `${v >= 0 ? '+' : ''}${v.toFixed(2)}`, 5);
  const settings = $derived(mixer.state!.metering);
  const target = $derived(settings.loudness.target);
  const tolerance = $derived(settings.loudness.tolerance);
  const l = $derived(reading?.loudness);
  const dev = $derived(l?.integrated == null ? null : l.integrated - target);
  const corr = $derived(reading?.correlation ?? null);
  /** Short-term loudness relative to the target on the Tech 3341 scale, 0..1. */
  const range = $derived(settings.scale === 'ebu18' ? { lo: -36, hi: 18 } : { lo: -18, hi: 9 });
  const rel = $derived(l?.shortTerm == null ? null : l.shortTerm - target);
  const at = (lu: number) => Math.min(1, Math.max(0, (lu - range.lo) / (range.hi - range.lo)));
</script>

<div class="loud" role="group" aria-label="Loudness">
  <div
    class="bar"
    title={`Short-term loudness against ${target} LUFS, ${settings.scale === 'ebu18' ? '-36 to +18' : '-18 to +9'} LU`}
    aria-hidden="true"
  >
    <!-- Bar-wide boxes moved and scaled by transform: a new target,
         tolerance or scale changes no layout. -->
    <i class="band" style:transform={`translateX(${at(-tolerance) * 100}%) scaleX(${at(tolerance) - at(-tolerance)})`}></i>
    <i class="fill" class:over={rel !== null && rel > tolerance} style:transform={`scaleX(${rel === null ? 0 : at(rel)})`}></i>
    <i class="target" style:transform={`translateX(${at(0) * 100}%)`}><b></b></i>
  </div>
  <span class="k">M</span><b class="num v"><span>{fmt(l?.momentary)}</span></b><b class="num max" title="Maximum momentary since reset"><span>{fmt(l?.maxMomentary)}</span></b>
  <span class="k">S</span><b class="num v"><span>{fmt(l?.shortTerm)}</span></b><b class="num max" title="Maximum short-term since reset"><span>{fmt(l?.maxShortTerm)}</span></b>
  <span class="k">I</span>
  <b
    class="num v int"
    class:ok={dev !== null && Math.abs(dev) <= tolerance}
    class:off={dev !== null && Math.abs(dev) > tolerance}
    class:invalid={l && !l.integratedValid}
    data-testid={`loudness-i-${id}`}><span>{fmt(l?.integrated)}</span></b
  >
  <b class="num max" title={`Target ${target} LUFS +/-${tolerance} LU`}><span>{fmt(target)}</span></b>
  <span class="k">LRA</span>
  <b class="num v" class:unstable={l !== undefined && !l.lraStable} title={l && !l.lraStable ? 'Settles after 60 s of measurement' : undefined}><span>{fmt(l?.lra)}</span></b>
  <span class="max unit">LU</span>
  <span class="k">TP</span><b class="num v" class:hot={tp !== null && tp > settings.zones.truePeakMax}><span>{fmt(tp)}</span></b><span
    class="max unit lamp"
    class:over
    data-testid={`tp-over-${id}`}
    title={over ? `True peak went above ${settings.zones.truePeakMax} dBTP since the last reset` : `No true peak above ${settings.zones.truePeakMax} dBTP since the last reset`}
    >dBTP<span class="sr-only">{over ? ', over since the last reset' : ''}</span></span
  >
  <!-- Every master keeps the correlation row, so all masters line up. -->
  {#if stereo}
    <span class="k" title="Phase correlation">COR</span>
    <b class="num v" class:hot={corr !== null && corr < 0}><span>{signed(corr)}</span></b>
  {:else}
    <span class="k blank" aria-hidden="true">COR</span>
    <b class="num v" aria-hidden="true"><span></span></b>
  {/if}
  <span class="max unit"></span>
  {#if onreset}
    <button class="key reset" aria-disabled={mixer.locked} onclick={reset} title="Reset integrated loudness, LRA, maxima and the true-peak over">RESET</button>
  {/if}
</div>

<style>
  /* Fixed tracks: no value, however long, resizes a column. Five
     characters ("-99.9", "-inf", "--.-") fit a value column. */
  .loud {
    display: grid;
    grid-template-columns: 20px 31px 31px;
    justify-content: space-between;
    column-gap: 3px;
    row-gap: 1px;
    align-items: center;
    background: var(--well);
    border-radius: 2px;
    padding: 3px 4px;
    color: #8b96a4;
    font-size: 10px;
  }
  .bar {
    grid-column: 1 / -1;
    position: relative;
    height: 6px;
    margin-bottom: 2px;
    background: #1b2129;
    overflow: hidden;
  }
  .bar i {
    position: absolute;
    top: 0;
    bottom: 0;
  }
  .band {
    left: 0;
    width: 100%;
    transform-origin: left;
    background: color-mix(in srgb, var(--meter-low) 30%, transparent);
  }
  .fill {
    left: 0;
    width: 100%;
    background: var(--meter-loud);
    transform-origin: left;
  }
  .fill.over {
    background: var(--meter-mid);
  }
  .target {
    left: 0;
    width: 100%;
  }
  .target b {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 1px;
    background: #e6edf3;
  }
  .k {
    font-weight: 600;
    letter-spacing: 0.04em;
  }
  /* Values change with every update: each sits in a block of fixed size with
     size and layout containment, so a longer or shorter text lays out
     nothing outside it. */
  b {
    font-weight: 400;
    min-width: 0;
  }
  b span {
    display: block;
    white-space: pre;
    height: 13px;
    line-height: 13px;
    text-align: right;
    contain: size layout style;
  }
  .v {
    color: #e6edf3;
    font-size: 10px;
  }
  .max {
    color: #8b96a4;
    font-size: 10px;
    text-align: right;
  }
  .unit {
    font-size: 9px;
  }
  .int.ok {
    color: var(--meter-low);
  }
  .int.off {
    color: var(--meter-mid);
  }
  .int.invalid span {
    text-decoration: line-through;
  }
  .blank {
    visibility: hidden;
  }
  .unstable {
    opacity: 0.55;
    font-style: italic;
  }
  .hot {
    color: var(--meter-high);
  }
  /* Latched true-peak over: lit until RESET. */
  .lamp {
    border-radius: 2px;
    padding: 0 1px;
  }
  .lamp.over {
    background: var(--meter-high);
    color: #fff;
  }
  .reset {
    grid-column: 1 / -1;
    margin-top: 2px;
    height: 20px;
    font-size: 10px;
  }
</style>
