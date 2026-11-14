<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { monitorPath } from '../lib/state/paths';
  import type { MeterGroup } from '../lib/meters/store';
  import Fader from './Fader.svelte';
  import Meter from './Meter.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const source = $derived(mixer.value<string>(monitorPath('source')));
  const listening = $derived(
    shape.channels.filter((c) => mixer.state!.channels[c.id]!.solo !== 'off').map((c) => ({ name: c.name, mode: mixer.state!.channels[c.id]!.solo })),
  );
  const sources = $derived([
    ...shape.buses.map((e) => ({ ...e, group: 'buses' as MeterGroup })),
    ...shape.auxes.map((e) => ({ ...e, group: 'auxes' as MeterGroup })),
    ...shape.mixMinus.map((e) => ({ ...e, group: 'mixMinus' as MeterGroup })),
  ]);
  const current = $derived(sources.find((s) => s.id === source));
  const flag = (key: 'dim' | 'cut' | 'mono') => mixer.value<boolean>(monitorPath(key));
</script>

<section class="monitor" aria-label="Monitor">
  <div class="panel-title" title={shape.monitorOutputs.map((o) => `${o.name} (${o.layout})`).join(', ')}>Monitor</div>
  <div class="listen" class:active={listening.length > 0} aria-live="polite">
    {#if listening.length > 0}
      <b>{listening.some((l) => l.mode === 'pfl') ? 'PFL' : 'AFL'}</b>
      <span>{listening.map((l) => l.name).join(', ')}</span>
    {:else}
      <span class="idle">{current?.name ?? source}</span>
    {/if}
  </div>
  <div class="sources" role="radiogroup" aria-label="Monitor source">
    {#each sources as s (s.id)}
      <button
        class="key"
        role="radio"
        aria-checked={s.id === source}
        onclick={() => mixer.set(monitorPath('source'), s.id)}
        title={s.name}>{s.name}</button
      >
    {/each}
  </div>
  <div class="body">
    <div class="f"><Fader path={monitorPath('level')} label="Monitor level" /></div>
    <div class="meter">
      {#if current}
        {#key current.id}
          <Meter group={current.group} id={current.id} legs={current.width} label={`Monitor source ${current.name} meter`} />
        {/key}
      {/if}
    </div>
    <div class="flags">
      <button class="key" aria-pressed={flag('dim')} onclick={() => mixer.set(monitorPath('dim'), !flag('dim'))}>DIM</button>
      <button class="key mute" aria-pressed={flag('cut')} onclick={() => mixer.set(monitorPath('cut'), !flag('cut'))}>CUT</button>
      <button class="key" aria-pressed={flag('mono')} onclick={() => mixer.set(monitorPath('mono'), !flag('mono'))}>MONO</button>
    </div>
  </div>
</section>

<style>
  .monitor {
    width: 150px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 6px;
    background: var(--panel-2);
    border-left: 1px solid var(--line-strong);
    min-height: 0;
  }
  .listen {
    min-height: 34px;
    border-radius: 3px;
    background: var(--well);
    color: #8b96a4;
    font-size: 11px;
    padding: 3px 6px;
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  .listen.active {
    background: var(--listen);
    color: #1a1200;
  }
  .listen span {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sources {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
  }
  .sources .key {
    font-size: 9.5px;
    letter-spacing: 0.02em;
    padding: 0 3px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .body {
    flex: 1;
    min-height: 150px;
    display: grid;
    grid-template-columns: auto 14px 1fr;
    gap: 6px;
  }
  .meter {
    padding: 17px 0;
  }
  .flags {
    display: flex;
    flex-direction: column;
    gap: 4px;
    justify-content: flex-end;
  }
  .flags .key {
    height: 30px;
  }
</style>
