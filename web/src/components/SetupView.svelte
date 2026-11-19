<script lang="ts">
  import { pendingWrite } from '../lib/pending';
  import { loudnessUnit, setLoudnessUnit } from '../lib/loudnessUnit.svelte';
  import type { LoudnessPreset, LoudnessScale } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import NumberEntry from './NumberEntry.svelte';
  import { setTheme, theme, type Theme } from '../lib/theme.svelte';

  const mixer = getMixer();
  const metering = $derived(mixer.state!.metering);
  const PRESETS: Array<{ id: LoudnessPreset; label: string; hint: string }> = [
    { id: 'ebu', label: 'EBU R 128', hint: '-23 LUFS +/-1 LU' },
    { id: 'atsc', label: 'ATSC A/85', hint: '-24 LKFS +/-2 LU' },
    { id: 'custom', label: 'Custom', hint: 'target and tolerance as entered' },
  ];
  const tone = $derived(mixer.state!.tone);
  const monitorState = $derived(mixer.state!.monitor);
  const outputs = $derived([...mixer.shape!.buses, ...mixer.shape!.auxes, ...mixer.shape!.mixMinus]);
  const TONE_LEVELS = [
    { level: -18, hint: 'EBU R 68 alignment' },
    { level: -20, hint: 'SMPTE alignment' },
  ];
  function toggleTarget(id: string): void {
    mixer.set('/tone/targets', tone.targets.includes(id) ? tone.targets.filter((t) => t !== id) : [...tone.targets, id]);
  }
  const THEMES: Array<{ id: Theme; label: string }> = [
    { id: 'dark', label: 'DARK' },
    { id: 'light', label: 'LIGHT' },
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
        <button {@attach pendingWrite(mixer, '/metering/loudness/preset')}
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
    <div class="radios" role="radiogroup" aria-label="Loudness readout on this screen">
      {#each ['LUFS', 'LU'] as const as u (u)}
        <button
          class="key"
          role="radio"
          aria-checked={loudnessUnit.value === u}
          title={u === 'LU' ? 'Relative to the target' : 'Absolute'}
          onclick={() => setLoudnessUnit(u)}>{u}</button
        >
      {/each}
    </div>
    <div class="radios" role="radiogroup" aria-label="Loudness meter scale">
      {#each SCALES as s (s.id)}
        <button {@attach pendingWrite(mixer, '/metering/scale')}
          class="key"
          role="radio"
          aria-checked={metering.scale === s.id}
          aria-disabled={mixer.locked}
          onclick={() => mixer.set('/metering/scale', s.id)}>{s.label}</button
        >
      {/each}
    </div>
  </section>

  <section class="card" aria-labelledby="setup-tone">
    <h2 class="panel-title" id="setup-tone">Line-up tone 1 kHz</h2>
    <div class="radios">
      <button {@attach pendingWrite(mixer, '/tone/on')} class="key tone-on" aria-disabled={mixer.locked} aria-pressed={tone.on} aria-label="Line-up tone" onclick={() => mixer.set('/tone/on', !tone.on)}>TONE {tone.on ? 'ON' : 'OFF'}</button>
      <button {@attach pendingWrite(mixer, '/tone/ident')} class="key" aria-disabled={mixer.locked} aria-pressed={tone.ident} onclick={() => mixer.set('/tone/ident', !tone.ident)} title="Stops the tone on one leg at a time to identify the legs">IDENT</button>
    </div>
    <div class="radios" role="radiogroup" aria-label="Tone level">
      {#each TONE_LEVELS as l (l.level)}
        <button {@attach pendingWrite(mixer, '/tone/level')} class="key" role="radio" aria-disabled={mixer.locked} aria-checked={tone.level === l.level} title={l.hint} onclick={() => mixer.set('/tone/level', l.level)}>{l.level} dBFS</button>
      {/each}
    </div>
    <NumberEntry path="/tone/level" label="Level" min={-30} max={0} step={1} unit="dBFS" />
    <div class="targets" role="group" aria-label="Tone outputs">
      {#each outputs as o (o.id)}
        <button {@attach pendingWrite(mixer, '/tone/targets')} class="key" aria-disabled={mixer.locked} aria-pressed={tone.targets.includes(o.id)} aria-label={`Tone on ${o.name}`} title={o.name} onclick={() => toggleTarget(o.id)}>{o.name}</button>
      {/each}
    </div>
  </section>

  <section class="card" aria-labelledby="setup-monitor">
    <h2 class="panel-title" id="setup-monitor">Monitor</h2>
    <NumberEntry path="/monitor/dimDepth" label="Dim depth" min={0} max={60} step={1} unit="dB" />
    <NumberEntry path="/monitor/monoCompensation" label="Mono compensation" min={0} max={6} step={0.5} unit="dB" />
    <div class="radios" role="radiogroup" aria-label="Monitor downmix of wider sources">
      <button {@attach pendingWrite(mixer, '/monitor/downmix')} class="key" role="radio" aria-disabled={mixer.locked} aria-checked={monitorState.downmix === 'loRo'} title="ITU-R BS.775 Lo/Ro" onclick={() => mixer.set('/monitor/downmix', 'loRo')}>Lo/Ro</button>
      <button {@attach pendingWrite(mixer, '/monitor/downmix')} class="key" role="radio" aria-disabled={mixer.locked} aria-checked={monitorState.downmix === 'lr'} title="Left and right legs only" onclick={() => mixer.set('/monitor/downmix', 'lr')}>L/R only</button>
    </div>
  </section>

  <section class="card" aria-labelledby="setup-screen">
    <h2 class="panel-title" id="setup-screen">This screen</h2>
    <!-- The same choice as the account menu's theme entry. -->
    <div class="radios" role="radiogroup" aria-label="Theme of this screen">
      {#each THEMES as t (t.id)}
        <button class="key" role="radio" aria-checked={theme.value === t.id} onclick={() => setTheme(t.id)}>{t.label}</button>
      {/each}
    </div>
  </section>

  <section class="card" aria-labelledby="setup-zones">
    <h2 class="panel-title" id="setup-zones">Meter zones</h2>
    <!-- Alignment stays below the permitted maximum, so the colours never invert. -->
    <NumberEntry path="/metering/zones/alignment" label="Alignment" min={-40} max={metering.zones.permittedMax - 1} step={1} unit="dBFS" />
    <NumberEntry path="/metering/zones/permittedMax" label="Permitted max" min={metering.zones.alignment + 1} max={0} step={1} unit="dBFS" />
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
  .targets {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 3px;
  }
  .targets .key {
    height: 28px;
  }
  .tone-on[aria-pressed='true'] {
    background: var(--mute);
    border-color: var(--mute);
    color: var(--mute-ink);
  }
  .hint {
    margin: 4px 0 0;
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
