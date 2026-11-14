<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { formatPan } from '../lib/audio/db';

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

  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
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
    else if (e.key === 'Home') v = -1;
    else if (e.key === 'End') v = 1;
    else if (e.key === '0') v = 0;
    if (v === null) return;
    e.preventDefault();
    mixer.set(path, Math.round(Math.max(-1, Math.min(1, v)) * 100) / 100);
  }
</script>

<div
  class="pan"
  role="slider"
  tabindex="0"
  aria-label={label}
  aria-valuemin={-1}
  aria-valuemax={1}
  aria-valuenow={value}
  aria-valuetext={formatPan(value, left, right)}
  title="Drag; Shift for fine; double-click to centre"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  ondblclick={() => mixer.set(path, 0)}
  onkeydown={key}
>
  <div class="bar">
    <i class="centre"></i>
    <i class="fill" style:left={`${Math.min(50, 50 + value * 50)}%`} style:right={`${Math.min(50, 50 - value * 50)}%`}></i>
    <i class="dot" style:left={`${50 + value * 50}%`}></i>
  </div>
  <span class="num">{formatPan(value, left, right)}</span>
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
  .fill {
    position: absolute;
    top: 1px;
    bottom: 1px;
    background: color-mix(in srgb, var(--afl) 55%, transparent);
    border-radius: 2px;
  }
  .dot {
    position: absolute;
    top: 50%;
    width: 6px;
    height: 12px;
    margin: -6px 0 0 -3px;
    background: var(--cap);
    border-radius: 2px;
  }
  span {
    font-size: 10px;
    color: var(--text-dim);
    text-align: right;
  }
</style>
