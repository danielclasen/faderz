<script lang="ts">
  import type { BusState, ChannelState, EqBandType, RecallGroup, Solo, StereoImage, Tap } from '../lib/contract/types';
  import { TAPS, TAP_KEY, TAP_LABEL } from '../lib/audio/taps';
  import { LAYOUT_LABELS, RECALL_GROUPS } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { pointer } from '../lib/state/pointer';
  import { recallSafePath } from '../lib/state/paths';
  import { formatDb, formatFreq } from '../lib/audio/db';
  import Param from './Param.svelte';
  import DelayEntry from './DelayEntry.svelte';
  import Pan from './Pan.svelte';
  import EqCurve from './EqCurve.svelte';

  interface Props {
    id: string;
    onclose: () => void;
  }
  let { id, onclose }: Props = $props();
  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const channel = $derived(shape.channels.find((c) => c.id === id));
  const group = $derived(channel ? 'channels' : shape.buses.some((b) => b.id === id) ? 'buses' : 'auxes');
  const element = $derived(channel ?? shape.buses.find((b) => b.id === id) ?? shape.auxes.find((a) => a.id === id)!);
  const st = $derived(mixer.state![group][id] as ChannelState | BusState);
  const ch = $derived(channel ? (st as ChannelState) : null);
  const p = (...rest: Array<string | number>) => pointer(group, id, ...rest);
  const flag = (...rest: Array<string | number>) => mixer.value<boolean>(p(...rest));
  const toggle = (...rest: Array<string | number>) => mixer.set(p(...rest), !flag(...rest));
  const safe = $derived(mixer.state!.recallSafe[id] ?? []);
  const IMAGES: Array<{ id: StereoImage; key: string; label: string }> = [
    { id: 'normal', key: 'L R', label: 'Normal' },
    { id: 'leftToBoth', key: 'L>B', label: 'Left to both' },
    { id: 'rightToBoth', key: 'R>B', label: 'Right to both' },
    { id: 'swap', key: 'SWP', label: 'Swap left and right' },
  ];
  const BAND_TYPES: EqBandType[] = ['lowShelf', 'peak', 'highShelf'];
  const typeLabel: Record<EqBandType, string> = { lowShelf: 'Low shelf', peak: 'Peak', highShelf: 'High shelf' };

  const legs = $derived(LAYOUT_LABELS[element.layout]);
  /** All legs inverted, or all back to normal when every leg already is. */
  function invertAll(): void {
    const target = !st.polarity.every(Boolean);
    st.polarity.forEach((v, i) => {
      if (v !== target) mixer.set(p('polarity', i), target);
    });
  }

  function toggleSafe(g: RecallGroup): void {
    mixer.set(recallSafePath(id), safe.includes(g) ? safe.filter((x) => x !== g) : [...safe, g]);
  }
  function toggleVca(vcaId: string): void {
    const members = mixer.state!.vcas[vcaId]!.members;
    mixer.set(pointer('vcas', vcaId, 'members'), members.includes(id) ? members.filter((m) => m !== id) : [...members, id]);
  }
  function keydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') onclose();
  }
</script>

<svelte:window onkeydown={keydown} />

<section class="detail" aria-label={`${element.name} processing`}>
  <header>
    <h2>{element.name}</h2>
    <span class="meta">{element.layout} - {group === 'channels' ? 'input channel' : group === 'buses' ? 'bus' : 'aux'}</span>
    <button class="key close" onclick={onclose} aria-label="Close processing">CLOSE</button>
  </header>
  <div class="cols">
    <div class="col">
      <h3 class="panel-title">Input</h3>
      {#if channel?.altInput}
        <div class="taps" role="radiogroup" aria-label={`${element.name} input source`}>
          <span class="lbl">Source</span>
          {#each ['a', 'b'] as const as src (src)}
            <button class="key" role="radio" aria-disabled={mixer.locked} aria-checked={ch?.input === src} aria-label={`Input ${src.toUpperCase()}`} onclick={() => mixer.set(p('input'), src)}>{src.toUpperCase()}</button>
          {/each}
        </div>
      {/if}
      {#if ch}
        <Param path={p('trim')} label="Trim" min={-20} max={20} unit=" dB" />
      {/if}
      {#if ch && element.width > 1}
        {#each legs as leg, i (i)}
          <Param path={p('legTrim', i)} label={`Trim ${leg}`} min={-20} max={20} unit=" dB" />
        {/each}
      {/if}
      <div class="polarity" role="group" aria-label={`${element.name} polarity per leg`}>
        <span class="lbl">Pol</span>
        {#each legs as leg, i (i)}
          <button class="key" aria-disabled={mixer.locked} aria-pressed={st.polarity[i]} onclick={() => toggle('polarity', i)} aria-label={`${element.name} ${leg} polarity invert`}>{leg}</button>
        {/each}
        {#if element.width > 1}
          <button class="key" aria-disabled={mixer.locked} aria-pressed={st.polarity.every(Boolean)} onclick={invertAll} aria-label={`${element.name} all legs polarity invert`}>ALL</button>
        {/if}
      </div>
      {#if ch}
        <div class="row">
          <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('hpf', 'enabled')} onclick={() => toggle('hpf', 'enabled')}>HPF</button>
        </div>
      {/if}
      {#if ch}
        <Param path={p('hpf', 'freq')} label="HPF" min={20} max={500} step={1} log format={(v) => `${formatFreq(v)} Hz`} />
      {/if}
      <DelayEntry path={p('delay')} label="Delay" />
      {#if ch}
        {#each [['meterTap', 'Meter'], ['directOut', 'Direct']] as const as [key, caption] (key)}
          {@const path = key === 'meterTap' ? p('meterTap') : p('directOut', 'tap')}
          <div class="taps" role="radiogroup" aria-label={`${element.name} ${caption === 'Meter' ? 'meter' : 'direct out'} pick-up point`}>
            <span class="lbl">{caption}</span>
            {#each TAPS as tap (tap)}
              <button
                class="key"
                role="radio"
                aria-disabled={mixer.locked}
                aria-checked={mixer.value<Tap>(path) === tap}
                aria-label={TAP_LABEL[tap]}
                onclick={() => mixer.set(path, tap)}>{TAP_KEY[tap]}</button
              >
            {/each}
          </div>
        {/each}
      {/if}
      {#if ch && element.width <= 2}
        <div class="pan-row"><span class="lbl">Pan</span><Pan path={p('pan', 'lr')} label={`${element.name} pan`} /></div>
        <div class="pan-row"><span class="lbl">F/R</span><Pan path={p('pan', 'fr')} label={`${element.name} front rear`} left="F" right="R" /></div>
        <Param path={p('pan', 'divergence')} label="Diverg" min={0} max={1} step={0.01} format={(v) => `${Math.round(v * 100)}%`} />
        <Param path={p('pan', 'lfe')} label="LFE" min={-90} max={10} format={(v) => formatDb(v)} />
      {:else if ch}
        <p class="note">{element.layout} maps leg to leg on {element.layout} buses and folds down (Lo/Ro) on narrower ones.</p>
      {/if}
      {#if ch && element.layout === 'stereo'}
        <div class="taps" role="radiogroup" aria-label={`${element.name} stereo image`}>
          <span class="lbl">Image</span>
          {#each IMAGES as im (im.id)}
            <button class="key" role="radio" aria-disabled={mixer.locked} aria-checked={ch.image === im.id} aria-label={im.label} title={im.label} onclick={() => mixer.set(p('image'), im.id)}>{im.key}</button>
          {/each}
        </div>
      {/if}
    </div>

    <div class="col eq">
      <div class="row">
        <h3 class="panel-title">EQ</h3>
        <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('eq', 'enabled')} onclick={() => toggle('eq', 'enabled')}>EQ IN</button>
      </div>
      <EqCurve eq={st.eq} hpf={ch?.hpf ?? null} width={300} height={96} grid />
      <div class="bands">
        {#each st.eq.bands as band, i (i)}
          <div class="band">
            <select
              disabled={mixer.locked}
              aria-label={`Band ${i + 1} type`}
              value={band.type}
              onchange={(e) => mixer.set(p('eq', 'bands', i, 'type'), (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each BAND_TYPES as t (t)}<option value={t}>{typeLabel[t]}</option>{/each}
            </select>
            <Param path={p('eq', 'bands', i, 'freq')} label="Freq" min={20} max={20000} step={1} log format={(v) => `${formatFreq(v)}`} />
            <Param path={p('eq', 'bands', i, 'gain')} label="Gain" min={-18} max={18} format={(v) => formatDb(v)} />
            <Param path={p('eq', 'bands', i, 'q')} label="Q" min={0.1} max={16} step={0.1} log format={(v) => v.toFixed(2)} />
          </div>
        {/each}
      </div>
    </div>

    <div class="col">
      <div class="row">
        <h3 class="panel-title">Gate</h3>
        <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('gate', 'enabled')} onclick={() => toggle('gate', 'enabled')}>IN</button>
      </div>
      <Param path={p('gate', 'threshold')} label="Thr" min={-80} max={0} unit=" dB" />
      <Param path={p('gate', 'range')} label="Range" min={0} max={80} unit=" dB" />
      <Param path={p('gate', 'attack')} label="Att" min={0.1} max={200} log unit=" ms" />
      <Param path={p('gate', 'release')} label="Rel" min={5} max={5000} step={1} log unit=" ms" />
      <div class="row">
        <h3 class="panel-title">Compressor</h3>
        <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('comp', 'enabled')} onclick={() => toggle('comp', 'enabled')}>IN</button>
      </div>
      <Param path={p('comp', 'threshold')} label="Thr" min={-60} max={0} unit=" dB" />
      <Param path={p('comp', 'ratio')} label="Ratio" min={1} max={20} log format={(v) => `${v.toFixed(1)}:1`} />
      <Param path={p('comp', 'attack')} label="Att" min={0.1} max={200} log unit=" ms" />
      <Param path={p('comp', 'release')} label="Rel" min={5} max={5000} step={1} log unit=" ms" />
      <Param path={p('comp', 'makeup')} label="Gain" min={0} max={24} unit=" dB" />
      <div class="row">
        <span class="lbl">Order</span>
        <button
          class="key"
          aria-disabled={mixer.locked}
          aria-pressed={flag('dynamicsBeforeEq')}
          onclick={() => toggle('dynamicsBeforeEq')}
          title="Lit: gate and compressor before EQ">{flag('dynamicsBeforeEq') ? 'DYN > EQ' : 'EQ > DYN'}</button
        >
      </div>
      {#if !ch}
        <div class="row">
          <h3 class="panel-title">Limiter</h3>
          <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('limiter', 'enabled')} onclick={() => toggle('limiter', 'enabled')} aria-label={`${element.name} true-peak limiter`}>IN</button>
        </div>
        <Param path={p('limiter', 'ceiling')} label="Ceil" min={-10} max={0} unit=" dBTP" />
      {/if}
    </div>

    {#if ch}
      <div class="col">
        <h3 class="panel-title">Sends</h3>
        {#each shape.auxes as aux (aux.id)}
          <div class="send">
            <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('sends', aux.id, 'on')} onclick={() => toggle('sends', aux.id, 'on')}>{aux.name}</button>
            <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('sends', aux.id, 'pre')} onclick={() => toggle('sends', aux.id, 'pre')} title="Pre-fader when lit">PRE</button>
            <Param path={p('sends', aux.id, 'level')} label="Lvl" min={-90} max={10} format={(v) => formatDb(v)} />
          </div>
        {/each}
        <h3 class="panel-title">Assign</h3>
        <div class="grid">
          {#each shape.buses as bus (bus.id)}
            <button class="key" aria-disabled={mixer.locked} aria-pressed={flag('routes', bus.id)} onclick={() => toggle('routes', bus.id)}>{bus.name}</button>
          {/each}
          {#each shape.vcas as vca (vca.id)}
            <button class="key" aria-disabled={mixer.locked} aria-pressed={mixer.state!.vcas[vca.id]!.members.includes(id)} onclick={() => toggleVca(vca.id)}>VCA {vca.name}</button>
          {/each}
        </div>
        <h3 class="panel-title">Listen</h3>
        <div class="grid">
          {#each ['off', 'pfl', 'afl'] as const as mode (mode)}
            <button class="key" aria-disabled={mixer.locked} aria-pressed={mixer.value<Solo>(p('solo')) === mode} onclick={() => mixer.set(p('solo'), mode)}>{mode}</button>
          {/each}
        </div>
      </div>
    {/if}

    <div class="col">
      <h3 class="panel-title">Recall safe</h3>
      <div class="grid">
        {#each RECALL_GROUPS as g (g)}
          {#if ch || !['pan', 'sends', 'routing', 'afv'].includes(g)}
            <button class="key" aria-disabled={mixer.locked} aria-pressed={safe.includes(g)} onclick={() => toggleSafe(g)}>{g}</button>
          {/if}
        {/each}
      </div>
    </div>
  </div>
</section>

<style>
  .detail {
    background: var(--panel);
    border-top: 2px solid var(--accent);
    max-height: 44vh;
    overflow: auto;
    padding: 6px 10px 10px;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 10px;
    position: sticky;
    top: -6px;
    background: var(--panel);
    padding: 4px 0;
    z-index: 1;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .meta {
    color: var(--text-dim);
    font-size: 11px;
  }
  .close {
    margin-left: auto;
  }
  .cols {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 5px;
    width: 230px;
  }
  .col.eq {
    width: 300px;
  }
  h3 {
    margin: 4px 0 0;
  }
  .row {
    display: flex;
    gap: 6px;
    align-items: center;
    justify-content: space-between;
  }
  .bands {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px 12px;
  }
  .band {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .band :global(.param) {
    grid-template-columns: 30px 1fr 40px;
  }
  .send {
    display: grid;
    grid-template-columns: 60px 36px 1fr;
    gap: 4px;
    align-items: center;
  }
  .send :global(.param) {
    grid-template-columns: 22px 1fr 40px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
  }
  .note {
    margin: 0;
    font-size: 11px;
    color: var(--text-dim);
  }
  .taps {
    display: grid;
    grid-template-columns: 52px;
    grid-auto-flow: column;
    grid-auto-columns: minmax(0, 1fr);
    gap: 3px;
    align-items: center;
  }
  .polarity {
    display: flex;
    gap: 3px;
    align-items: center;
    flex-wrap: wrap;
  }
  .polarity .lbl {
    width: 30px;
  }
  .polarity .key {
    min-width: 30px;
  }
  .pan-row {
    display: grid;
    grid-template-columns: 52px 1fr;
    gap: 6px;
    align-items: center;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
</style>
