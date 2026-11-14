<script lang="ts">
  import type { LoudnessPreset, LoudnessScale } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import NumberEntry from './NumberEntry.svelte';

  const mixer = getMixer();
  const metering = $derived(mixer.state!.metering);
  const PRESETS: Array<{ id: LoudnessPreset; label: string; hint: string }> = [
    { id: 'ebu', label: 'EBU R 128', hint: '-23 LUFS +/-1 LU' },
    { id: 'atsc', label: 'ATSC A/85', hint: '-24 LKFS +/-2 LU' },
    { id: 'custom', label: 'Custom', hint: 'target and tolerance as entered' },
  ];
  const SCALES: Array<{ id: LoudnessScale; label: string }> = [
    { id: 'ebu9', label: 'EBU +9' },
    { id: 'ebu18', label: 'EBU +18' },
  ];
</script>

<div class="view">
  <section class="card" aria-labelledby="setup-loudness">
    <h2 class="panel-title" id="setup-loudness">Loudness</h2>
    <div class="radios" role="radiogroup" aria-label="Loudness preset">
      {#each PRESETS as p (p.id)}
        <button
          class="key"
          role="radio"
          aria-checked={metering.loudness.preset === p.id}
          aria-disabled={mixer.locked}
          title={p.hint}
          onclick={() => mixer.set('/metering/loudness/preset', p.id)}>{p.label}</button
        >
      {/each}
    </div>
    <NumberEntry path="/metering/loudness/target" label="Target" min={-31} max={-14} step={0.5} unit="LUFS" />
    <NumberEntry path="/metering/loudness/tolerance" label="Tolerance" min={0.5} max={5} step={0.5} unit="LU" />
    <div class="radios" role="radiogroup" aria-label="Loudness meter scale">
      {#each SCALES as s (s.id)}
        <button
          class="key"
          role="radio"
          aria-checked={metering.scale === s.id}
          aria-disabled={mixer.locked}
          onclick={() => mixer.set('/metering/scale', s.id)}>{s.label}</button
        >
      {/each}
    </div>
  </section>

  <section class="card" aria-labelledby="setup-zones">
    <h2 class="panel-title" id="setup-zones">Meter zones</h2>
    <NumberEntry path="/metering/zones/alignment" label="Alignment" min={-40} max={0} step={1} unit="dBFS" />
    <NumberEntry path="/metering/zones/permittedMax" label="Permitted max" min={-40} max={0} step={1} unit="dBFS" />
    <NumberEntry path="/metering/zones/truePeakMax" label="True-peak max" min={-40} max={0} step={0.5} unit="dBTP" />
    <p class="hint">Shared by every client of this mixer.</p>
  </section>
</div>

<style>
  .view {
    display: flex;
    flex-wrap: wrap;
    align-content: flex-start;
    gap: 16px;
    padding: 10px 14px;
    height: 100%;
    overflow: auto;
    scrollbar-gutter: stable;
  }
  .card {
    width: 300px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 10px;
  }
  .radios {
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: 1fr;
    gap: 3px;
  }
  .radios .key {
    height: 28px;
  }
  .hint {
    margin: 4px 0 0;
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
