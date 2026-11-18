<script lang="ts">
  // Live operation of one duck group in the masters: its mode, the DUCK
  // key, what it does now in words, who holds it, its key level against
  // its threshold, and the threshold. Every part has a fixed size, so no
  // state change moves anything.
  import { onMount } from 'svelte';
  import { fit } from '../lib/format';
  import { tip } from '../lib/tip';
  import { LAMP_WIDTH, keyShare, lampLit, lampText, liveDuck } from '../lib/duck';
  import type { DuckMeter, DuckMode, DuckShape } from '../lib/contract/types';
  import { onAnimationFrame } from '../lib/meters/store';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { duckPath } from '../lib/state/paths';
  import { formatDb } from '../lib/audio/db';
  import DuckKey from './DuckKey.svelte';
  import DuckModes from './DuckModes.svelte';
  import Slider from './Slider.svelte';

  interface Props {
    duck: DuckShape;
    /** Shift+D+n presses its DUCK key. */
    shortcut: number;
  }
  let { duck, shortcut }: Props = $props();
  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const d = $derived(mixer.state!.ducks[duck.id]!);
  const modePath = $derived(duckPath(duck.id, 'mode'));
  const thresholdPath = $derived(duckPath(duck.id, 'threshold'));
  const mode = $derived(mixer.value<DuckMode>(modePath));
  const holder = $derived(d.hold ? mixer.ownerOf(duckPath(duck.id, 'hold')) : null);
  const holderText = $derived(holder ? `Held by ${mixer.ownerLabel(holder)}` : '');
  const names = (ids: string[]) => ids.map((id) => [...shape.channels, ...shape.buses, ...shape.auxes, ...shape.mixMinus].find((e) => e.id === id)?.name ?? id).join(', ') || 'none';
  const members = (key: 'trigger' | 'target') => Object.keys(d.channels).filter((c) => d.channels[c]![key]);
  /** The tile on demand: what keys the group, what it lowers, where, and how it stands. */
  const description = $derived(
    `${duck.name}: ${lampText(d).toLowerCase()}${holderText ? `, ${holderText.toLowerCase()}` : ''}. Keyed by ${names(members('trigger'))}; lowers ${names(members('target'))} in ${names(Object.keys(d.destinations).filter((k) => d.destinations[k]))}. Threshold ${formatDb(d.threshold)} dBFS.`,
  );

  // The group's readings, polled on the frame clock: each changes at most
  // once per meter frame, and only what changed renders.
  let key = $state<number | null>(null);
  let envelope = $state(0);
  let engaged = $state(false);
  onMount(() =>
    onAnimationFrame((now) => {
      const m = liveDuck(mixer.meters, duck.id, now);
      key = m?.key ?? null;
      envelope = m?.envelope ?? 0;
      engaged = m?.engaged ?? false;
    }),
  );
  const live = $derived<DuckMeter | null>(key === null && envelope === 0 && !engaged ? null : { key, envelope, engaged });
  const lit = $derived(lampLit(d, live));
  /** The key level on the threshold's scale, -60..0 dBFS, 0..1. */
  const keyAt = $derived(keyShare(key));
</script>

<section class="tile" aria-label={`Duck ${duck.name}`} data-duck={duck.id}>
  <div class="top">
    <span class="name" role="note" aria-label={description} use:tip={description}>{duck.name}</span>
    <!-- Who holds the duck, in a slot kept whether held or not. -->
    <span class="holder" class:held={holder !== null} class:mine={mixer.isMine(holder)} role="note" aria-label={holderText || undefined} use:tip={holderText} data-testid={`duck-holder-${duck.id}`}
      ><span>{holder ? mixer.ownerShort(holder) : ''}</span></span
    >
  </div>
  <!-- Not a live region: with the envelope it changes per syllable. -->
  <div class="lamp" class:lit class:bypass={!lit && mode === 'off'} data-testid={`duck-lamp-${duck.id}`}>
    <span class="fixed">{fit(lampText(d, live), LAMP_WIDTH)}</span>
  </div>
  <DuckModes id={duck.id} name={duck.name} />
  <DuckKey id={duck.id} name={duck.name} {shortcut} />
  <div class="keymeter" aria-hidden="true" data-testid={`duck-keymeter-${duck.id}`}>
    <i class="key" class:over={engaged} style:transform={`scaleX(${keyAt})`} data-testid={`duck-key-level-${duck.id}`}></i>
    <span class="travel" style:transform={`translateX(${keyShare(d.threshold) * 100}%)`}><i class="marker"></i></span>
  </div>
  <div class="sens">
    <span class="lbl">THR</span>
    <Slider
      path={thresholdPath}
      label={`${duck.name} duck threshold`}
      toPos={(v) => (v + 60) / 60}
      fromPos={(p) => Math.round((p * 60 - 60) * 2) / 2}
      valueText={`${formatDb(d.threshold)} dBFS`}
      keyStep={1 / 60}
    />
    <span class="thr fixed" data-testid={`duck-threshold-${duck.id}`}>{fit(formatDb(d.threshold), 5)}</span>
  </div>
</section>

<style>
  .tile {
    width: 140px;
    height: var(--tile-h, auto);
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 5px 6px 6px;
    background: var(--panel-2);
    border-left: 1px solid var(--line);
    box-sizing: border-box;
  }
  .top {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 46px;
    gap: 4px;
    align-items: center;
    height: 18px;
  }
  .name {
    font-size: 12px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border-radius: 2px;
  }
  .holder {
    height: 14px;
    border-radius: 2px;
    font-size: 10px;
    font-weight: 600;
    text-align: center;
    color: #fff;
  }
  .holder span {
    display: block;
    height: 14px;
    line-height: 14px;
    padding: 0 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    contain: strict;
  }
  .holder.held {
    background: var(--owned);
    color: var(--owned-ink);
  }
  .holder.held.mine {
    background: var(--on);
    color: var(--on-ink);
  }
  /* The text changes with the state; a block of fixed size with strict
     containment lays out nothing outside it. */
  .lamp {
    height: 20px;
    border-radius: 2px;
    border: 1px solid var(--line-strong);
    color: var(--text-dim);
    font-size: 11px;
    font-weight: 600;
    text-align: center;
  }
  .lamp span {
    display: block;
    height: 18px;
    line-height: 18px;
    contain: strict;
    color: inherit;
  }
  .lamp.lit {
    background: var(--duck);
    border-color: var(--duck);
    color: var(--duck-ink);
  }
  /* Bypassed: a dashed edge as well as the word, so it reads without colour. */
  .lamp.bypass {
    border-style: dashed;
    border-color: var(--text-dim);
    background-image: var(--hatch);
    color: var(--text);
  }
  /* The key level against the threshold: a well of fixed size, the
     marker moved by transform. */
  .keymeter {
    position: relative;
    height: 8px;
    border-radius: 2px;
    background: var(--well);
    box-shadow: inset 0 0 0 1px var(--line);
    overflow: clip;
  }
  .keymeter .travel {
    position: absolute;
    inset: 0 2px 0 0;
  }
  /* The key level, scaled from the left on the travel of the marker: at
     the threshold it ends where the marker starts. */
  .keymeter .key {
    position: absolute;
    inset: 1px 2px 1px 0;
    transform-origin: left;
    background: var(--meter-low);
  }
  .keymeter .key.over {
    background: var(--duck);
  }
  .keymeter .marker {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--meter-tp);
  }
  /* The threshold slider: 24 px tall to a mouse, 32 px to a finger, its
     track and cap centred in it. */
  .sens :global(.slider) {
    height: 24px;
  }
  .sens :global(.slider .track),
  .sens :global(.slider .fill) {
    top: 10px;
  }
  .sens :global(.slider .thumb) {
    top: 4px;
  }
  @media (pointer: coarse) {
    .sens :global(.slider) {
      height: 32px;
    }
    .sens :global(.slider .track),
    .sens :global(.slider .fill) {
      top: 14px;
    }
    .sens :global(.slider .thumb) {
      top: 5px;
    }
  }
  .sens {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 4px;
    align-items: center;
  }
  .lbl {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-faint);
    letter-spacing: 0.06em;
  }
  .thr {
    font-size: 10px;
    color: var(--text-dim);
    line-height: 18px;
  }
</style>
