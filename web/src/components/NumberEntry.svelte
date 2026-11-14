<script lang="ts">
  // Typed entry of one numeric control: Enter or leaving the field writes
  // the value, clamped to min..max and rounded to step; Escape restores the
  // current value. The field never resizes with its text.
  import { getMixer } from '../lib/state/mixer.svelte';

  interface Props {
    path: string;
    label: string;
    min: number;
    max: number;
    step?: number;
    unit?: string;
  }
  let { path, label, min, max, step = 0.1, unit = '' }: Props = $props();
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
    const v = Number((Math.round(Math.min(max, Math.max(min, typed)) / step) * step).toFixed(decimals));
    input.value = v.toFixed(decimals);
    if (v !== value) mixer.set(path, v);
  }
  function keydown(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      commit();
      input?.select();
    } else if (e.key === 'Escape' && input) {
      input.value = shown;
      input.blur();
    }
  }
</script>

<label class="entry">
  <span class="lbl">{label}</span>
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
    aria-label={`${label}, ${min} to ${max}${unit ? ` ${unit}` : ''}`}
    data-path={path}
  />
  <span class="unit">{unit}</span>
</label>

<style>
  .entry {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 7ch 4ch;
    gap: 6px;
    align-items: center;
    min-height: 26px;
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
    color: #e6edf3;
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
