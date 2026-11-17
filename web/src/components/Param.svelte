<script lang="ts">
  import { fit } from '../lib/format';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { parseEntry } from '../lib/audio/entry';
  import { dbToPosition, positionToDb } from '../lib/audio/db';
  import Slider from './Slider.svelte';

  interface Props {
    path: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    unit?: string;
    log?: boolean;
    /** A level on the fader taper (lib/audio/db.ts), as faders and the Aux view: most travel around 0 dB. */
    fader?: boolean;
    format?: (v: number) => string;
  }
  let { path, label, min, max, step = 0.1, unit = '', log = false, fader = false, format }: Props = $props();

  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path));
  const foreign = $derived(mixer.ownedByOther(path));
  const id = `p-${Math.random().toString(36).slice(2, 9)}`;

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

</script>

<div class="param" class:foreign>
  <span class="label" id={id}>{label}</span>
  <Slider
    {path}
    {label}
    toPos={(v) => (fader ? dbToPosition(v) : log ? Math.log(v / min) / Math.log(max / min) : (v - min) / (max - min))}
    fromPos={(p) => {
      if (fader) return Number(Math.min(max, Math.max(min, positionToDb(p))).toFixed(4));
      const v = log ? min * Math.pow(max / min, p) : min + p * (max - min);
      return Number((Math.round(Math.min(max, Math.max(min, v)) / step) * step).toFixed(4));
    }}
    valueText={shown}
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
    grid-template-columns: 52px 1fr auto;
    align-items: center;
    gap: 6px;
    min-height: 22px;
  }
  .label {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Nine characters of the value font, the longest value ("-0.5 dBTP"),
     plus the padding and the border, so no value or unit is cut. */
  .entry {
    width: calc(9ch + 8px);
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
