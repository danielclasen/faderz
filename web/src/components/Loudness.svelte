<script lang="ts">
  import { pendingWrite } from '../lib/pending';
  import { fit, levelText } from '../lib/format';
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import type { MeterGroup, MeterReading } from '../lib/meters/store';
  import { loudnessUnit } from '../lib/loudnessUnit.svelte';
  import { hold } from '../lib/hold';
  import { pointer } from '../lib/state/pointer';
  import { loudnessNote } from '../lib/loudnessNote';
  import { showTip, tip } from '../lib/tip';

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
  const RESET_HOLD_MS = 1000;
  /** Pausing a running integration loses the programme while paused: it takes a hold. Starting is one press. */
  const PAUSE_HOLD_MS = 500;
  const mixer = getMixer();
  let reading = $state<MeterReading | undefined>();
  /** Highest true peak since the last reset, dBTP, as the over lamp latches it. */
  let tp = $state<number | null>(null);

  // 10 readouts per second, the rate EBU Tech 3341 asks of a live meter.
  // TP is the maximum true-peak level since the last reset (EBU R 128),
  // gathered from every frame, so a short over between two ticks stays
  // readable.
  onMount(() => {
    const t = setInterval(() => {
      reading = mixer.meters.get(group, id);
      tp = mixer.meters.truePeakSinceReset(group, id);
    }, READOUT_MS);
    return () => clearInterval(t);
  });
  const over = $derived(mixer.meters.overs.has(`${group}/${id}`));
  function reset(): void {
    if (mixer.locked) return;
    mixer.meters.clearOvers(group, [id]);
    onreset?.();
  }

  const runPath = $derived(pointer('loudness', id, 'running'));
  /** The integrated measurement takes in audio (EBU Tech 3341 start and pause). */
  const running = $derived(mixer.value<boolean | undefined>(runPath) !== false);
  /**
   * Fixed targets, chosen when the press starts (hold captures fire at
   * press): a hold that began on a running integration pauses it, even if
   * another screen paused it meanwhile.
   */
  function pause(): void {
    if (!mixer.locked) mixer.set(runPath, false);
  }
  function run(): void {
    if (!mixer.locked) mixer.set(runPath, true);
  }

  /** One decimal place at most (EBU Tech 3341); every value fits 5 characters. */
  const fmt = (v: number | null | undefined) => levelText(v);
  const signed = (v: number | null | undefined) => fit(v === null || v === undefined ? '--.--' : `${v >= 0 ? '+' : ''}${v.toFixed(2)}`, 5);
  const settings = $derived(mixer.state!.metering);
  const target = $derived(settings.loudness.target);
  /** Loudness values as read on this screen: LUFS, or LU relative to the target. */
  const relative = $derived(loudnessUnit.value === 'LU');
  const lu = (v: number | null | undefined) => levelText(v, relative ? target : undefined);
  const tolerance = $derived(settings.loudness.tolerance);
  const l = $derived(reading?.loudness);
  const dev = $derived(l?.integrated == null ? null : l.integrated - target);
  const corr = $derived(reading?.correlation ?? null);
  /** Short-term loudness relative to the target on the Tech 3341 scale, 0..1. */
  const range = $derived(settings.scale === 'ebu18' ? { lo: -36, hi: 18 } : { lo: -18, hi: 9 });
  const rel = $derived(l?.shortTerm == null ? null : l.shortTerm - target);
  /** Gaps and a restarted start of the measurement, from the serving replica's counters. */
  const note = $derived(loudnessNote(mixer.loudnessHealth[id]));
  const at = (lu: number) => Math.min(1, Math.max(0, (lu - range.lo) / (range.hi - range.lo)));
</script>

<div class="loud" role="group" aria-label={relative ? `Loudness in LU relative to ${target} LUFS` : "Loudness in LUFS"}>
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
  <span class="k">M</span><b class="num v"><span>{lu(l?.momentary)}</span></b><b class="num max" class:held={!running} title={`Maximum momentary since reset${running ? '' : ', held while paused'}`}><span>{lu(l?.maxMomentary)}</span></b>
  <span class="k">S</span><b class="num v"><span>{lu(l?.shortTerm)}</span></b><b class="num max" class:held={!running} title={`Maximum short-term since reset${running ? '' : ', held while paused'}`}><span>{lu(l?.maxShortTerm)}</span></b>
  <span class="k">I</span>
  <b
    class="num v int"
    class:ok={dev !== null && Math.abs(dev) <= tolerance}
    class:off={dev !== null && Math.abs(dev) > tolerance}
    class:invalid={l && !l.integratedValid}
    class:held={!running}
    title={running ? undefined : 'Integration paused: integrated loudness, LRA and maxima hold until RUN'}
    data-testid={`loudness-i-${id}`}><span>{lu(l?.integrated)}</span></b
  >
  <b class="num max" title={`Target ${target} LUFS +/-${tolerance} LU`}><span>{fmt(target)}</span></b>
  <span class="k">LRA</span>
  <b class="num v" class:unstable={l !== undefined && !l.lraStable} class:held={!running} title={!running ? 'Integration paused' : l && !l.lraStable ? 'Settles after 60 s of measurement' : undefined}><span>{fmt(l?.lra)}</span></b>
  <span class="max unit">LU</span>
  <!-- Always there, so a note coming or going moves nothing; its text
       sits in a box of fixed size with size and layout containment. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <span
    class="note"
    role="note"
    tabindex={note.tip ? 0 : -1}
    aria-label={note.tip || undefined}
    data-testid={`loudness-note-${id}`}
    use:tip={note.tip}
    onpointerup={(e) => {
      if (note.tip) showTip(e.currentTarget, e.pointerType === 'touch');
    }}
    ><span class="text"><span class="gaps">{note.gaps ?? ''}</span><span class="since">{note.since ?? ''}</span></span></span
  >
  <span class="k">TP</span><b class="num v" class:hot={tp !== null && tp > settings.zones.truePeakMax} title="Maximum true peak since the last reset, measured while the integration is paused too" data-testid={`tp-${id}`}><span>{fmt(tp)}</span></b><span
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
    <div class="keys">
      <!-- Unlit while the integration runs; lit amber, reading PAUSED,
           while it is paused, so a forgotten pause shows at a glance.
           Pausing takes a hold, running on one press. -->
      <button
        {@attach pendingWrite(mixer, runPath)}
        class="key hold pause"
        data-testid={`loudness-pause-${id}`}
        aria-disabled={mixer.locked}
        aria-pressed={!running}
        use:hold={{ ms: running ? PAUSE_HOLD_MS : 0, fire: running ? pause : run }}
        aria-label={running ? 'Loudness integration running, hold to pause' : 'Loudness integration paused, press to run'}
        title={running ? 'Integrating: hold to pause integrated loudness, LRA and maxima' : 'Paused: press to run the integration on from its held values'}
        >{running ? 'PAUSE' : 'PAUSED'}</button
      >
      <!-- Resetting mid-programme loses the running measurement: it takes a hold. -->
      <button
        class="key hold reset"
        aria-disabled={mixer.locked}
        use:hold={{ ms: RESET_HOLD_MS, fire: reset }}
        aria-label="Reset loudness, hold for one second"
        title="Hold for one second to reset integrated loudness, LRA, maxima and the true-peak over">RESET</button
      >
    </div>
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
    font-size: 10px;
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
  .note {
    grid-column: 1 / -1;
    min-width: 0;
    border-radius: 2px;
  }
  .note:focus-visible {
    outline: 2px solid var(--focus);
  }
  /* Gaps at the left, the start at the right; the tooltip has both
     whole. */
  .note .text {
    display: flex;
    justify-content: space-between;
    gap: 4px;
    height: 12px;
    line-height: 12px;
    font-size: 10px;
    white-space: nowrap;
    contain: size layout style;
  }
  /* The count first: the start gives way, cut with an ellipsis. */
  .note .gaps {
    flex: none;
    color: var(--well-warn);
  }
  .note .since {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
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
  /* Held by a paused integration: a dashed amber edge, which takes no room. */
  .held span {
    outline: 1px dashed var(--well-warn);
    outline-offset: -1px;
  }
  .key.pause[aria-pressed='true'] {
    background: var(--paused);
    border-color: var(--paused);
    color: var(--paused-ink);
  }
  .keys {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
    margin-top: 2px;
  }
  .hold {
    position: relative;
    touch-action: none;
    height: 20px;
    padding: 0;
    font-size: 10px;
    letter-spacing: 0.02em;
  }
  @media (pointer: coarse) {
    .hold {
      height: 28px;
    }
  }
  .hold::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--text);
    transform: scaleX(0);
    transform-origin: left;
  }
  .hold:global(.arming)::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
  }
</style>
