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
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    drag = { x: e.clientX, v: value, id: e.pointerId, w: el.clientWidth };
    mixer.touch(path);
  }
  function move(e: PointerEvent): void {
    if (!drag || e.pointerId !== drag.id) return;
    const scale = e.shiftKey ? 0.1 : 1;
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
    const step = e.shiftKey ? 0.01 : 0.05;
    let v: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') v = value - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') v = value + step;
    // Jumps need Alt, as on a fader: a stray key never swings a source.
    else if (e.key === 'Home' && e.altKey) v = -1;
    else if (e.key === 'End' && e.altKey) v = 1;
    else if (e.key === '0' && e.altKey) v = 0;
    if (v === null) return;
    e.preventDefault();
    mixer.touch(path);
    const next = Math.round(Math.max(-1, Math.min(1, v)) * 100) / 100;
    if (next !== value) mixer.set(path, next);
    if (keyRelease) clearTimeout(keyRelease);
    keyRelease = setTimeout(() => mixer.release(path), 800);
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
  <!-- Fill and dot move by transform only, which changes no layout. -->
  <div class="bar">
    <i class="centre"></i>
    <i class="fill" style:transform={`scaleX(${value})`}></i>
    <i class="travel" style:transform={`translateX(${value * 50}%)`}><b class="dot"></b></i>
  </div>
  <span class="num fixed">{fit(formatPan(value, left, right), 4)}</span>
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
  .bar {
    position: relative;
    height: 8px;
    background: var(--well);
    border-radius: 4px;
    border: 1px solid var(--line);
  }
  .centre {
    position: absolute;
    left: 50%;
    top: -2px;
    bottom: -2px;
    width: 1px;
    background: var(--line-strong);
  }
  /* Spans the right half from the centre; scaleX(-1..1) mirrors it to the left. */
  .fill {
    position: absolute;
    top: 1px;
    bottom: 1px;
    left: 50%;
    width: 50%;
    transform-origin: left center;
    background: color-mix(in srgb, var(--afl) 55%, transparent);
    border-radius: 2px;
  }
  /* As wide as the bar: translateX(50%) puts its centre on the right end. */
  .travel {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .dot {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 6px;
    height: 12px;
    margin: -6px 0 0 -3px;
    background: var(--cap);
    border-radius: 2px;
  }
  /* The readout changes with every update: a block of fixed size with
     size and layout containment, so no text length lays out anything. */
  span {
    display: block;
    width: 30px;
    height: 13px;
    line-height: 13px;
    contain: size layout style;
    font-size: 10px;
    color: var(--text-dim);
    text-align: right;
  }
</style>
