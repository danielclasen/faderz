<script lang="ts">
  import { fit } from '../lib/format';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { parseEntry } from '../lib/audio/entry';

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

  let field: HTMLInputElement | undefined = $state();

  /** Writes a typed value, clamped and rounded like the slider's. */
  function commit(): void {
    if (!field) return;
    const typed = parseEntry(field.value, min);
    if (typed === null) {
      field.value = fit(shown, 8);
      return;
    }
    const v = Number((Math.round(Math.min(max, Math.max(min, typed)) / step) * step).toFixed(4));
    if (v !== value) mixer.set(path, v);
    field.value = fit(format ? format(v) : `${Number(v.toFixed(step < 1 ? 1 : 0))}${unit}`, 8);
  }
  function fieldKey(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      commit();
      field?.select();
    } else if (e.key === 'Escape' && field) {
      field.value = fit(shown, 8);
      field.blur();
    }
  }

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
    disabled={mixer.locked}
    oninput={input}
    onpointerdown={() => mixer.touch(path)}
    onpointerup={() => mixer.release(path)}
    onpointercancel={() => mixer.release(path)}
    onlostpointercapture={() => mixer.release(path)}
    aria-valuetext={shown}
    data-path={path}
  />
  <input
    bind:this={field}
    class="entry fixed"
    type="text"
    inputmode="decimal"
    value={fit(shown, 8)}
    disabled={mixer.locked}
    onchange={commit}
    onkeydown={fieldKey}
    onfocus={() => field?.select()}
    aria-label={`${label} value, type to set`}
    title="Type a value; Enter sets it, Escape restores"
  />
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
  .entry {
    width: 100%;
    box-sizing: border-box;
    height: 20px;
    padding: 0 3px;
    font-size: 11px;
    text-align: right;
    color: var(--text);
    background: transparent;
    border: 1px solid transparent;
    border-radius: 2px;
  }
  .entry:hover {
    border-color: var(--line-strong);
  }
  .entry:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 0;
    background: var(--well);
  }
  .foreign .entry {
    color: var(--owned);
  }
</style>
