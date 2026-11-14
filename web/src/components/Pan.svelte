<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { formatPan } from '../lib/audio/db';
  import { fit } from '../lib/format';

  interface Props {
    path: string;
    label: string;
    left?: string;
    right?: string;
  }
  let { path, label, left = 'L', right = 'R' }: Props = $props();
  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path) ?? 0);

  let drag: { x: number; v: number; id: number; w: number } | null = null;
  let keyRelease: ReturnType<typeof setTimeout> | null = null;

  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    e.preventDefault();
    if (mixer.refuseIfHeld(path)) return;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    drag = { x: e.clientX, v: value, id: e.pointerId, w: el.clientWidth };
    mixer.touch(path);
  }
  function move(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    const scale = e.shiftKey || mixer.fine ? 0.1 : 1;
    const v = Math.max(-1, Math.min(1, drag.v + ((e.clientX - drag.x) / drag.w) * 2 * scale));
    const r = Math.round(v * 100) / 100;
    if (r !== value) mixer.set(path, r);
  }
  function up(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    drag = null;
    mixer.release(path);
  }
  function key(e: KeyboardEvent): void {
    const step = e.shiftKey || mixer.fine ? 0.01 : 0.05;
    let v: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') v = value - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') v = value + step;
    // Jumps need Alt, as on a fader: a stray key never swings a source.
    else if (e.key === 'Home' && e.altKey) v = -1;
    else if (e.key === 'End' && e.altKey) v = 1;
    else if (e.key === '0' && e.altKey) v = 0;
    if (v === null) return;
    e.preventDefault();
    if (mixer.refuseIfHeld(path)) return;
    mixer.touch(path);
    const next = Math.round(Math.max(-1, Math.min(1, v)) * 100) / 100;
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

<div
  class="pan"
  role="slider"
  tabindex="0"
  aria-disabled={mixer.locked}
  aria-label={label}
  aria-valuemin={-1}
  aria-valuemax={1}
  aria-valuenow={value}
  aria-valuetext={formatPan(value, left, right)}
  title="Drag; Shift for fine; arrows step, Alt+0 centres, Alt+Home/End for full left/right"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onlostpointercapture={up}
  onkeydown={key}
  onblur={blur}
  data-path={path}
>
  <!-- Fill and dot are background layers of fixed boxes: a new value
       repaints them and changes no layout. Not transforms: each is a paint
       property node, and one per strip adds to every frame's work. -->
  <div class="bar" style:--a={`${Math.min(50, 50 + value * 50)}%`} style:--b={`${Math.max(50, 50 + value * 50)}%`}>
    <i class="centre"></i>
    <i class="dot" style:--f={value / 2 + 0.5}></i>
  </div>
  <span class="readout"><span class="num fixed">{fit(formatPan(value, left, right), 4)}</span></span>
</div>

<style>
  .pan {
    display: grid;
    grid-template-columns: 1fr 30px;
    align-items: center;
    gap: 4px;
    height: 18px;
    cursor: ew-resize;
    touch-action: none;
    border-radius: 2px;
  }
  @media (pointer: coarse) {
    .pan {
      height: 28px;
    }
  }
  .centre {
    position: absolute;
    left: 50%;
    top: -2px;
    bottom: -2px;
    width: 1px;
    background: var(--line-strong);
  }
  /* The fill runs from the centre to the value, as a gradient between --a and --b. */
  .bar {
    position: relative;
    height: 8px;
    border-radius: 4px;
    border: 1px solid var(--line);
    background-color: var(--well);
    background-image: linear-gradient(
      to right,
      transparent var(--a),
      color-mix(in srgb, var(--afl) 55%, transparent) var(--a),
      color-mix(in srgb, var(--afl) 55%, transparent) var(--b),
      transparent var(--b)
    );
    background-clip: padding-box;
  }
  /* A 6 x 12 px cap image whose centre sits at the fraction --f of the
     bar: a percentage position counts over the box minus the image. */
  .dot {
    position: absolute;
    left: -1px;
    right: -1px;
    top: -3px;
    bottom: -3px;
    pointer-events: none;
    background: linear-gradient(var(--cap), var(--cap)) no-repeat;
    background-size: 6px 100%;
    background-position-x: calc(var(--f) * 100% + (var(--f) * 6px - 3px));
  }
  /* The readout changes with every update. A grid item is not a relayout
     boundary, so the text sits in a block child of fixed size with strict
     containment, and is padded to one length, so no text moves anything. */
  .readout {
    width: 30px;
    font-size: 10px;
    color: var(--text-dim);
    text-align: right;
  }
  .readout span {
    display: block;
    height: 13px;
    line-height: 13px;
    contain: strict;
  }
</style>
