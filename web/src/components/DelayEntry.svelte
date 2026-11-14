<script lang="ts">
  // Delay of a channel or bus, typed in ms, frames or samples. Enter or
  // leaving the field writes it; Escape restores the current value; arrow
  // keys step one unit (Shift: a tenth). A suffix (ms, f, smp, s) overrides
  // the selected unit for one entry.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { formatDelay, fromSamples, parseDelay, MAX_DELAY_SAMPLES, type DelayUnit } from '../lib/audio/delay';

  interface Props {
    path: string;
    label: string;
  }
  let { path, label }: Props = $props();
  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const value = $derived(mixer.value<number>(path));
  const UNITS: Array<{ id: DelayUnit; key: string; name: string }> = [
    { id: 'ms', key: 'MS', name: 'milliseconds' },
    { id: 'frames', key: 'FR', name: 'frames' },
    { id: 'samples', key: 'SMP', name: 'samples' },
  ];
  let unit = $state<DelayUnit>('ms');
  const shown = $derived(formatDelay(value, unit, shape.sampleRate, shape.frameRate));
  const fpsText = $derived(
    shape.frameRate.denominator === 1 ? `${shape.frameRate.numerator}` : (shape.frameRate.numerator / shape.frameRate.denominator).toFixed(2),
  );
  let input: HTMLInputElement | undefined = $state();

  function write(samples: number): void {
    if (input) input.value = formatDelay(samples, unit, shape.sampleRate, shape.frameRate);
    if (samples !== value) mixer.set(path, samples);
  }
  function commit(): void {
    if (!input) return;
    const samples = parseDelay(input.value, unit, shape.sampleRate, shape.frameRate);
    if (samples === null) input.value = shown;
    else write(samples);
  }
  function keydown(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      commit();
      input?.select();
    } else if (e.key === 'Escape' && input) {
      input.value = shown;
      input.blur();
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      const one = fromSamples(1, unit, shape.sampleRate, shape.frameRate);
      const step = (e.shiftKey ? 0.1 : 1) / one;
      const next = Math.min(MAX_DELAY_SAMPLES, Math.max(0, Math.round(value + (e.key === 'ArrowUp' ? step : -step))));
      write(next);
    }
  }
</script>

<div class="delay">
  <label class="lbl" for={`${path}-entry`}>{label}</label>
  <input
    bind:this={input}
    id={`${path}-entry`}
    class="num"
    type="text"
    inputmode="decimal"
    value={shown}
    disabled={mixer.locked}
    onchange={commit}
    onkeydown={keydown}
    onfocus={() => input?.select()}
    aria-label={`${label} in ${UNITS.find((u) => u.id === unit)!.name}, up to ${Number((MAX_DELAY_SAMPLES / shape.sampleRate).toFixed(2))} seconds`}
    data-path={path}
  />
  <div class="units" role="radiogroup" aria-label={`${label} unit`}>
    {#each UNITS as u (u.id)}
      <button
        class="key"
        role="radio"
        aria-checked={unit === u.id}
        aria-label={u.name}
        title={u.id === 'frames' ? `Frames at ${fpsText} fps` : u.name}
        onclick={() => (unit = u.id)}>{u.key}</button
      >
    {/each}
  </div>
</div>

<style>
  .delay {
    display: grid;
    grid-template-columns: 52px 8ch minmax(0, 1fr);
    gap: 4px;
    align-items: center;
    min-height: 26px;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
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
  .units {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 2px;
  }
  .units .key {
    padding: 0;
    font-size: 10px;
  }
</style>
