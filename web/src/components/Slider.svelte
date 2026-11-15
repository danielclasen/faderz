<script lang="ts">
  // Horizontal control for a continuous parameter. A drag moves it by the
  // distance travelled, from wherever it was, like a fader: touching it
  // never jumps the value to the touch point, as a native range input
  // does. Shift or the FINE latch moves it at a tenth of the travel; the
  // arrows step, Page Up and Page Down step ten times as far.
  import { getMixer } from '../lib/state/mixer.svelte';

  interface Props {
    path: string;
    label: string;
    /** Value to travel position, 0..1. */
    toPos: (v: number) => number;
    /** Travel position, 0..1, to a value as written (rounded by the caller). */
    fromPos: (p: number) => number;
    /** Text for assistive technology. */
    valueText: string;
    /** Position step of an arrow key. */
    keyStep?: number;
  }
  let { path, label, toPos, fromPos, valueText, keyStep = 0.01 }: Props = $props();
  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path));
  const pos = $derived(Math.min(1, Math.max(0, toPos(value))));

  let drag: { x: number; p: number; id: number; w: number; fine: boolean } | null = null;
  const isFine = (e: PointerEvent | KeyboardEvent) => mixer.fine || e.shiftKey || e.ctrlKey || e.metaKey;

  function write(p: number): void {
    const v = fromPos(Math.min(1, Math.max(0, p)));
    if (v !== value) mixer.set(path, v);
  }
  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    e.preventDefault();
    if (mixer.locked || mixer.refuseIfHeld(path)) return;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    drag = { x: e.clientX, p: pos, id: e.pointerId, w: el.clientWidth, fine: isFine(e) };
    mixer.touch(path);
  }
  function move(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    const fine = isFine(e);
    if (fine !== drag.fine) {
      // Rebase on a modifier change so the value does not jump.
      drag = { ...drag, x: e.clientX, p: pos, fine };
      return;
    }
    write(drag.p + ((e.clientX - drag.x) / drag.w) * (fine ? 0.1 : 1));
  }
  function up(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    drag = null;
    mixer.release(path);
  }
  function key(e: KeyboardEvent): void {
    const step = isFine(e) ? keyStep / 10 : keyStep;
    const delta =
      e.key === 'ArrowRight' || e.key === 'ArrowUp' ? step
      : e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -step
      : e.key === 'PageUp' ? step * 10
      : e.key === 'PageDown' ? -step * 10
      : null;
    if (delta === null) return;
    e.preventDefault();
    if (mixer.locked || mixer.refuseIfHeld(path)) return;
    // The value is rounded to its resolution, so a step smaller than that
    // would round back to where it was, or, from a value off the grid,
    // behind it: widen the step until the value moves the key's way, so
    // every press moves by at least one step of the value, or not at all
    // at the end of the range.
    const at = (k: number) => Math.min(1, Math.max(0, pos + delta * k));
    const ahead = (k: number) => (delta > 0 ? fromPos(at(k)) > value : fromPos(at(k)) < value);
    let k = 1;
    while (!ahead(k) && at(k) !== at(k + 1)) k++;
    if (ahead(k)) write(at(k));
  }
</script>

<div
  class="slider"
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-disabled={mixer.locked}
  aria-valuemin={0}
  aria-valuemax={100}
  aria-valuenow={Math.round(pos * 100)}
  aria-valuetext={valueText}
  data-path={path}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onlostpointercapture={up}
  onkeydown={key}
>
  <i class="track"></i>
  <i class="fill" style:transform={`scaleX(${pos})`}></i>
  <!-- The thumb moves by transform, which changes no layout: the travel
       box spans the thumb's range, so its translation in percent is of
       that range. -->
  <span class="travel" style:transform={`translateX(${pos * 100}%)`}><i class="thumb"></i></span>
</div>

<style>
  .slider {
    position: relative;
    height: 18px;
    cursor: ew-resize;
    touch-action: none;
    border-radius: 2px;
  }
  .track,
  .fill {
    position: absolute;
    left: 0;
    right: 0;
    top: 7px;
    height: 4px;
    border-radius: 2px;
  }
  .track {
    background: var(--track);
    box-shadow: inset 0 0 0 1px var(--line);
  }
  .fill {
    background: color-mix(in srgb, var(--afl) 55%, transparent);
    transform-origin: left;
  }
  .travel {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: calc(100% - 10px);
    pointer-events: none;
  }
  .thumb {
    position: absolute;
    left: 0;
    top: 1px;
    width: 10px;
    height: 16px;
    border-radius: 2px;
    background: var(--cap);
    border: 1px solid var(--cap-edge);
    box-sizing: border-box;
  }
  @media (pointer: coarse) {
    .slider {
      height: 28px;
    }
    .track,
    .fill {
      top: 12px;
    }
    .travel {
      width: calc(100% - 14px);
    }
    .thumb {
      top: 3px;
      width: 14px;
      height: 22px;
    }
  }
</style>
