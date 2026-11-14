<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';

  interface Props {
    path: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    unit?: string;
    log?: boolean;
    format?: (v: number) => string;
  }
  let { path, label, min, max, step = 0.1, unit = '', log = false, format }: Props = $props();

  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path));
  const foreign = $derived(mixer.ownedByOther(path));
  const id = `p-${Math.random().toString(36).slice(2, 9)}`;

  // A log control maps the slider's 0..1000 range onto min..max exponentially.
  const toSlider = (v: number) => (log ? (Math.log(v / min) / Math.log(max / min)) * 1000 : v);
  const fromSlider = (s: number) => {
    if (!log) return Math.round(s / step) * step;
    const v = min * Math.pow(max / min, s / 1000);
    return Math.round(v / step) * step;
  };
  const shown = $derived(format ? format(value) : `${Number(value.toFixed(step < 1 ? 1 : 0))}${unit}`);

  function input(e: Event): void {
    const v = Math.min(max, Math.max(min, fromSlider(Number((e.currentTarget as HTMLInputElement).value))));
    mixer.set(path, Number(v.toFixed(4)));
  }
</script>

<div class="param" class:foreign>
  <label for={id}>{label}</label>
  <input
    {id}
    type="range"
    min={log ? 0 : min}
    max={log ? 1000 : max}
    step={log ? 1 : step}
    value={toSlider(value)}
    oninput={input}
    onpointerdown={() => mixer.touch(path)}
    onpointerup={() => mixer.release(path)}
    onpointercancel={() => mixer.release(path)}
    aria-valuetext={shown}
    data-path={path}
  />
  <output for={id} class="num">{shown}</output>
</div>

<style>
  .param {
    display: grid;
    grid-template-columns: 52px 1fr 54px;
    align-items: center;
    gap: 6px;
    min-height: 22px;
  }
  label {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  output {
    font-size: 11px;
    text-align: right;
    color: var(--text);
  }
  .foreign output {
    color: var(--owned);
  }
</style>
