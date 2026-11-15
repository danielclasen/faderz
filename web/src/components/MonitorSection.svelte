<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { monitorPath } from '../lib/state/paths';
  import { pointer } from '../lib/state/pointer';
  import { LAYOUT_LABELS, type StereoImage } from '../lib/contract/types';
  import Fader from './Fader.svelte';
  import Meter from './Meter.svelte';
  import { tip } from '../lib/tip';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const source = $derived(mixer.value<string>(monitorPath('source')));
  const listening = $derived([
    ...shape.vcas.filter((v) => mixer.state!.vcas[v.id]!.solo !== 'off').map((v) => ({ name: `VCA ${v.name}`, mode: mixer.state!.vcas[v.id]!.solo })),
    ...shape.channels.filter((c) => mixer.state!.channels[c.id]!.solo !== 'off').map((c) => ({ name: c.name, mode: mixer.state!.channels[c.id]!.solo })),
  ]);
  // PFL takes priority over AFL: while any PFL is on, the AFL listens wait.
  const pfl = $derived(listening.filter((l) => l.mode === 'pfl'));
  const afl = $derived(listening.filter((l) => l.mode === 'afl'));
  const listenMode = $derived(pfl.length > 0 ? 'PFL' : afl.length > 0 ? 'AFL' : null);
  const heard = $derived((pfl.length > 0 ? pfl : afl).map((l) => l.name).join(', '));
  const listenText = $derived(
    listenMode === null ? '' : `${listenMode}: ${heard}${pfl.length > 0 && afl.length > 0 ? `; AFL held while PFL is on: ${afl.map((l) => l.name).join(', ')}` : ''}`,
  );
  const sources = $derived([
    ...shape.buses,
    ...shape.auxes,
    ...shape.mixMinus,
  ]);
  const current = $derived(sources.find((s) => s.id === source));
  const output = $derived(shape.monitorOutputs[0]);
  const legs = $derived(output ? LAYOUT_LABELS[output.layout] : []);
  const monitor = $derived(mixer.state!.monitor);
  const IMAGES: Array<{ id: StereoImage; key: string; label: string }> = [
    { id: 'leftToBoth', key: 'L>B', label: 'Monitor left to both' },
    { id: 'rightToBoth', key: 'R>B', label: 'Monitor right to both' },
    { id: 'swap', key: 'SWP', label: 'Monitor swap left and right' },
  ];
  const flag = (key: 'dim' | 'cut' | 'mono' | 'talkbackDim') => mixer.value<boolean>(monitorPath(key));
</script>

<section class="monitor" id="monitor" tabindex="-1" aria-label="Monitor">
  <div class="panel-title" title={shape.monitorOutputs.map((o) => `${o.name} (${o.layout})`).join(', ')}>Monitor</div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="listen"
    class:active={listening.length > 0}
    class:afl={listenMode === 'AFL'}
    aria-live="polite"
    role="status"
    tabindex={listening.length > 0 ? 0 : -1}
    use:tip={listenText}
  >
    {#if listenMode}
      <b>{listenMode}{pfl.length > 0 && afl.length > 0 ? ' (AFL held)' : ''}</b>
      <span>{heard}</span>
    {:else}
      <span class="idle">{current?.name ?? source}</span>
    {/if}
  </div>
  <div class="sources" role="radiogroup" aria-label="Monitor source">
    {#each sources as s (s.id)}
      <button
        class="key"
        role="radio"
        aria-disabled={mixer.locked}
        aria-checked={s.id === source}
        onclick={() => mixer.set(monitorPath('source'), s.id)}
        title={s.name}>{s.name}</button
      >
    {/each}
  </div>
  <div class="speakers" role="group" aria-label="Monitor speakers" style:grid-template-columns={`repeat(${legs.length}, minmax(0, 1fr))`}>
    {#each legs as leg, i (i)}
      <button class="key mute" class:pending={mixer.pendingKey(pointer('monitor', 'speakerMute', i))} aria-disabled={mixer.locked} aria-pressed={monitor.speakerMute[i]} aria-label={`Mute speaker ${leg}`} title={`Mute speaker ${leg}`} onclick={() => mixer.set(pointer('monitor', 'speakerMute', i), !monitor.speakerMute[i])}>{leg}</button>
    {/each}
    {#each legs as leg, i (i)}
      <button class="key solo" class:pending={mixer.pendingKey(pointer('monitor', 'speakerSolo', i))} aria-disabled={mixer.locked} aria-pressed={monitor.speakerSolo[i]} aria-label={`Solo speaker ${leg}`} title={`Solo speaker ${leg}`} onclick={() => mixer.set(pointer('monitor', 'speakerSolo', i), !monitor.speakerSolo[i])}>S</button>
    {/each}
  </div>
  {#if output?.layout === 'stereo'}
    <div class="image" role="radiogroup" aria-label="Monitor image">
      {#each IMAGES as im (im.id)}
        <button
          class="key"
          role="radio"
          aria-disabled={mixer.locked}
          aria-checked={monitor.image === im.id}
          aria-label={im.label}
          title={im.label}
          onclick={() => mixer.set(monitorPath('image'), monitor.image === im.id ? 'normal' : im.id)}>{im.key}</button
        >
      {/each}
    </div>
  {/if}
  <div class="body">
    <div class="f"><Fader path={monitorPath('level')} label="Monitor level" /></div>
    <!-- What the monitor plays: the listen sum while anyone listens, else the source. -->
    <div class="meter">
      {#if output}
        <Meter
          group="monitor"
          id={output.id}
          legs={output.width}
          label={listenMode ? `Monitor meter, ${listenMode} listen sum` : `Monitor meter, source ${current?.name ?? source}`}
        />
      {/if}
    </div>
    <div class="flags">
      <button class="key" class:pending={mixer.pendingKey(monitorPath('dim'))} aria-disabled={mixer.locked} aria-pressed={flag('dim')} onclick={() => mixer.set(monitorPath('dim'), !flag('dim'))}>DIM</button>
      <button class="key mute" class:pending={mixer.pendingKey(monitorPath('cut'))} aria-disabled={mixer.locked} aria-pressed={flag('cut')} onclick={() => mixer.set(monitorPath('cut'), !flag('cut'))}>CUT</button>
      <button class="key" class:pending={mixer.pendingKey(monitorPath('mono'))} aria-disabled={mixer.locked} aria-pressed={flag('mono')} onclick={() => mixer.set(monitorPath('mono'), !flag('mono'))}>MONO</button>
      <button
        class="key"
        class:pending={mixer.pendingKey(monitorPath('talkbackDim'))}
        aria-disabled={mixer.locked}
        aria-pressed={flag('talkbackDim')}
        aria-label="Dim the monitor while talkback is active"
        title="Dim the monitor while talkback is active"
        onclick={() => mixer.set(monitorPath('talkbackDim'), !flag('talkbackDim'))}>TB DIM</button
      >
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
  .listen.active.afl {
    background: var(--afl);
    color: var(--afl-ink);
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
    font-size: 10px;
    letter-spacing: 0.02em;
    padding: 0 3px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .speakers,
  .image {
    display: grid;
    gap: 2px;
  }
  .image {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
  .speakers .key,
  .image .key {
    padding: 0;
    font-size: 10px;
  }
  .speakers .solo[aria-pressed='true'] {
    background: var(--listen);
    border-color: var(--listen);
    color: #1a1200;
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
