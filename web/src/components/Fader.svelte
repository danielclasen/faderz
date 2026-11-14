<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { FADER_MARKS, FADER_OFF_DB, dbToPosition, dragDb, formatDb, keyStepDb, stepDb } from '../lib/audio/db';

  interface Props {
    path: string;
    label: string;
    scale?: boolean;
  }
  let { path, label, scale = true }: Props = $props();

  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path) ?? FADER_OFF_DB);
  const owner = $derived(mixer.ownerOf(path));
  const mine = $derived(mixer.isMine(owner));
  const foreign = $derived(owner !== null && !mine);
  const ownerName = $derived(mixer.ownerLabel(owner));
  const position = $derived(dbToPosition(value));

  let flash = $state(false);
  /** Cap height: the cap's centre travels the track height minus this. */
  const CAP_H = 34;
  /** The cap's travel is read once per drag: reading it per move forces a layout. */
  let drag: { y: number; db: number; fine: boolean; id: number; height: number } | null = null;
  let queued: number | null = null;
  let frame = 0;
  let keyRelease: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const at = mixer.conflicts.get(path);
    if (!at) return;
    flash = true;
    const t = setTimeout(() => (flash = false), 700);
    return () => clearTimeout(t);
  });

  function send(db: number): void {
    queued = db;
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (queued !== null && queued !== mixer.value<number>(path)) mixer.set(path, queued);
      queued = null;
    });
  }

  const isFine = (e: PointerEvent | KeyboardEvent) => e.shiftKey || e.ctrlKey || e.metaKey;

  function pointerdown(e: PointerEvent): void {
    if (e.button !== 0) return;
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    drag = { y: e.clientY, db: value, fine: isFine(e), id: e.pointerId, height: (e.currentTarget as HTMLElement).clientHeight - CAP_H };
    mixer.touch(path);
  }

  function pointermove(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    const fine = isFine(e);
    if (fine !== drag.fine) {
      // Rebase on a modifier change so the cap does not jump.
      drag = { ...drag, y: e.clientY, db: queued ?? value, fine };
      return;
    }
    send(dragDb(drag.db, e.clientY - drag.y, drag.height, fine));
  }

  function pointerup(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    drag = null;
    if (frame) {
      cancelAnimationFrame(frame);
      frame = 0;
      if (queued !== null) mixer.set(path, queued);
      queued = null;
    }
    mixer.release(path);
  }

  function keydown(e: KeyboardEvent): void {
    let next: number | null = null;
    const fine = isFine(e);
    switch (e.key) {
      case 'ArrowUp':
      case 'ArrowRight':
        next = stepDb(value, fine ? 0.1 : keyStepDb(value));
        break;
      case 'ArrowDown':
      case 'ArrowLeft':
        next = stepDb(value, fine ? -0.1 : -keyStepDb(value));
        break;
      case 'PageUp':
        next = stepDb(value, 5);
        break;
      case 'PageDown':
        next = stepDb(value, -5);
        break;
      // Jumps need Alt: a single stray key never opens or closes a fader on air.
      case 'End':
        if (!e.altKey) return;
        next = FADER_OFF_DB;
        break;
      case '0':
        if (!e.altKey) return;
        next = 0;
        break;
      default:
        return;
    }
    e.preventDefault();
    mixer.touch(path);
    if (next !== value) mixer.set(path, next);
    if (keyRelease) clearTimeout(keyRelease);
    keyRelease = setTimeout(() => {
      keyRelease = null;
      mixer.release(path);
    }, 800);
  }

  function blur(): void {
    if (keyRelease) {
      clearTimeout(keyRelease);
      keyRelease = null;
      mixer.release(path);
    }
  }
</script>

<div class="fader" class:flash>
  {#if scale}
    <div class="scale" aria-hidden="true">
      {#each FADER_MARKS as mark (mark)}
        <span style:bottom="calc({dbToPosition(mark)} * (100% - 34px) + 17px)" class:unity={mark === 0}>
          {mark <= FADER_OFF_DB ? '-' : mark > 0 ? `+${mark}` : mark}
        </span>
      {/each}
    </div>
  {/if}
  <div
    class="track"
    role="slider"
    tabindex="0"
    aria-disabled={mixer.locked}
    aria-label={label}
    aria-valuemin={FADER_OFF_DB}
    aria-valuemax={10}
    aria-valuenow={value}
    aria-valuetext={`${formatDb(value)} dB${foreign ? `, held by ${ownerName}` : ''}`}
    title={foreign ? `Held by ${ownerName}` : 'Drag; Shift for fine; arrows step, Alt+0 for 0 dB, Alt+End closes'}
    onpointerdown={pointerdown}
    onpointermove={pointermove}
    onpointerup={pointerup}
    onpointercancel={pointerup}
    onlostpointercapture={pointerup}
    onkeydown={keydown}
    onblur={blur}
    data-path={path}
  >
    <div class="slot"></div>
    {#if owner}
      <span class="owner" class:mine data-testid="owner" data-owner-kind={owner.kind}>{ownerName}</span>
    {/if}
    {#each FADER_MARKS as mark (mark)}
      <i class="tick" class:unity={mark === 0} style:bottom="calc({dbToPosition(mark)} * (100% - 34px) + 17px)"></i>
    {/each}
    <div class="travel" style:transform="translateY({(1 - position) * 100}%)">
      <div class="cap" class:mine class:foreign><span></span></div>
    </div>
  </div>
</div>

<style>
  .fader {
    display: flex;
    height: 100%;
    min-height: 120px;
    gap: 2px;
  }
  @media (max-height: 500px) {
    .fader {
      min-height: 90px;
    }
  }
  .scale {
    position: relative;
    width: 20px;
    flex: none;
  }
  .scale span {
    position: absolute;
    right: 0;
    /* Centred on its mark by margin, not transform: a transform is a paint
       property node per element, and every one adds to each frame's
       layerization. */
    height: 1em;
    margin-bottom: -0.5em;
    font-family: var(--font-num);
    font-size: 8.5px;
    color: var(--text-faint);
    line-height: 1;
  }
  .scale span.unity {
    color: var(--text-dim);
    font-weight: 600;
  }
  .owner {
    position: absolute;
    top: 0;
    left: 50%;
    transform: translateX(-50%);
    z-index: 2;
    max-width: 60px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    pointer-events: none;
    font-size: 8px;
    line-height: 11px;
    padding: 0 2px;
    border-radius: 2px;
    background: var(--owned);
    color: #fff;
  }
  .owner.mine {
    background: var(--accent);
  }
  .track {
    position: relative;
    width: 30px;
    flex: none;
    cursor: ns-resize;
    touch-action: none;
    border-radius: 3px;
  }
  .slot {
    position: absolute;
    left: 50%;
    top: 12px;
    bottom: 12px;
    width: 4px;
    margin-left: -2px;
    background: var(--track);
    border-radius: 2px;
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .tick {
    position: absolute;
    left: 3px;
    right: 3px;
    height: 1px;
    margin-bottom: -0.5px;
    background: var(--line-strong);
  }
  .tick.unity {
    background: var(--text-faint);
    left: 0;
    right: 0;
  }
  /* The cap moves by transform, which changes no layout. The travel box
     spans the cap's range: its translation in percent is of that range. */
  .travel {
    position: absolute;
    inset: 0 0 34px;
    pointer-events: none;
  }
  .cap {
    position: absolute;
    top: 0;
    left: 1px;
    right: 1px;
    height: 34px;
    border-radius: 3px;
    background: linear-gradient(180deg, color-mix(in srgb, var(--cap) 92%, white) 0%, var(--cap) 45%, color-mix(in srgb, var(--cap) 80%, black) 100%);
    border: 1px solid var(--cap-edge);
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.45);
    pointer-events: none;
  }
  .cap span {
    position: absolute;
    left: 3px;
    right: 3px;
    top: 50%;
    height: 2px;
    margin-top: -1px;
    background: #10141a;
  }
  .cap.mine {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 55%, transparent), 0 2px 4px rgba(0, 0, 0, 0.45);
  }
  .cap.foreign {
    border-color: var(--owned);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--owned) 60%, transparent);
  }
  .flash .cap {
    animation: refuse 0.7s ease-out;
  }
  @media (prefers-reduced-motion: reduce) {
    .flash .cap {
      box-shadow: 0 0 0 3px var(--err);
    }
  }
  @keyframes refuse {
    0%,
    40% {
      box-shadow: 0 0 0 3px var(--err);
    }
    100% {
      box-shadow: 0 0 0 0 transparent;
    }
  }
</style>
