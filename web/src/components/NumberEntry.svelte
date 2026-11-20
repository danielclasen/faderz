<script lang="ts">
  // Typed entry of one numeric control: Enter or leaving the field writes
  // the value, rounded to step; one outside min..max is never clamped: the
  // field is restored and the range shown as an error. Escape restores the
  // current value. The field never resizes with its text.
  import { getMixer } from '../lib/state/mixer.svelte';

  interface Props {
    path: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    unit?: string;
    /** Label only for assistive technology, e.g. in a table with column headers. */
    compact?: boolean;
    /** Tab order of the field, for a grid with one tab stop. */
    tabindex?: number;
    /** Runs first on every key; a key it prevents is not handled here. */
    onnav?: (e: KeyboardEvent) => void;
  }
  let { path, label, min, max, step = 0.1, unit = '', compact = false, tabindex, onnav }: Props = $props();
  const mixer = getMixer();
  const value = $derived(mixer.value<number>(path));
  const decimals = $derived(step >= 1 ? 0 : Math.min(2, Math.ceil(-Math.log10(step))));
  const shown = $derived(value.toFixed(decimals));
  let input: HTMLInputElement | undefined = $state();

  function commit(): void {
    if (!input) return;
    const typed = Number(input.value.replace(',', '.'));
    if (!Number.isFinite(typed)) {
      input.value = shown;
      return;
    }
    if (typed < min || typed > max) {
      input.value = shown;
      mixer.notify('error', `${label} takes ${min} to ${max}${unit ? ` ${unit}` : ''}`, path);
      return;
    }
    const v = Number((Math.round(typed / step) * step).toFixed(decimals));
    input.value = v.toFixed(decimals);
    if (v !== value) mixer.set(path, v);
  }
  function keydown(e: KeyboardEvent): void {
    onnav?.(e);
    if (e.defaultPrevented) return;
    if (e.key === 'Enter') {
      commit();
      input?.select();
    } else if (e.key === 'Escape' && input) {
      input.value = shown;
      input.blur();
    }
  }
</script>

<label class="entry" class:compact>
  {#if !compact}<span class="lbl">{label}</span>{/if}
  <input
    bind:this={input}
    class="num"
    type="text"
    inputmode="decimal"
    value={shown}
    disabled={mixer.locked}
    onchange={commit}
    onkeydown={keydown}
    onfocus={() => input?.select()}
    {tabindex}
    aria-label={`${label}, ${min} to ${max}${unit ? ` ${unit}` : ''}`}
    data-path={path}
  />
  {#if !compact}<span class="unit">{unit}</span>{/if}
</label>

<style>
  .entry {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 7ch 4ch;
    gap: 6px;
    align-items: center;
    min-height: 26px;
  }
  .entry.compact {
    grid-template-columns: 7ch;
  }
  .lbl {
    font-size: 12px;
    color: var(--text-dim);
  }
  input {
    width: 100%;
    box-sizing: border-box;
    height: 24px;
    text-align: right;
    font-size: 12px;
    background: var(--well);
    color: var(--text);
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    padding: 0 5px;
  }
  input:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 0;
  }
  .unit {
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
