<script lang="ts">
  import type { ChannelShape, ChannelState, Solo } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { channelPath } from '../lib/state/paths';
  import { effectiveGainDb, formatDb } from '../lib/audio/db';
  import Fader from './Fader.svelte';
  import Meter from './Meter.svelte';
  import Pan from './Pan.svelte';
  import EqCurve from './EqCurve.svelte';

  interface Props {
    channel: ChannelShape;
    index: number;
    selected: boolean;
    onselect: (id: string) => void;
  }
  let { channel, index, selected, onselect }: Props = $props();

  const mixer = getMixer();
  const st = $derived(mixer.state!.channels[channel.id] as ChannelState);
  const afv = $derived(mixer.state!.afv[channel.id]);
  const tally = $derived(afv?.source ? mixer.state!.tally[afv.source] : undefined);
  const mute = $derived(mixer.value<boolean>(channelPath(channel.id, 'mute')));
  const solo = $derived(mixer.value<Solo>(channelPath(channel.id, 'solo')));
  const fader = $derived(mixer.value<number>(channelPath(channel.id, 'fader')));
  const vcas = $derived(
    (mixer.shape?.vcas ?? []).filter((v) => mixer.state!.vcas[v.id]?.members.includes(channel.id)),
  );
  const vcaMuted = $derived(vcas.some((v) => mixer.state!.vcas[v.id]!.mute));
  const effective = $derived(effectiveGainDb(fader, vcas.map((v) => mixer.state!.vcas[v.id]!.offset)));
  const badge = $derived(channel.layout === 'mono' ? '' : channel.layout === 'stereo' ? 'ST' : channel.layout);
  const short = (bus: { name: string; kind: string }) =>
    bus.kind === 'cleanFeed' ? 'CF' : bus.kind === 'me' ? 'ME' : (bus.name.split(' ').at(-1) ?? bus.name).slice(0, 4);

  function toggleSolo(mode: Exclude<Solo, 'off'>): void {
    mixer.set(channelPath(channel.id, 'solo'), solo === mode ? 'off' : mode);
  }
</script>

<section class="strip" class:selected class:alt={index % 2 === 1} aria-label={`Channel ${index + 1} ${channel.name}`} data-channel={channel.id}>
  <button class="head" onclick={() => onselect(channel.id)} aria-pressed={selected} title="Open channel processing">
    <span class="no num">{index + 1}</span>
    {#if badge}<span class="badge">{badge}</span>{/if}
    <span class="name">{channel.name}</span>
  </button>

  <div class="afv" class:program={tally === 'program'} class:preview={tally === 'preview'} class:closed={afv?.enabled && !afv.open}>
    {#if afv?.enabled && afv.source}
      <span title={`Audio follows video: ${afv.source} ${tally ?? 'no tally'}`}>AFV {afv.source}</span>
    {:else}
      <span class="none">-</span>
    {/if}
  </div>

  <div class="proc" aria-label="Processing">
    <span class:on={st.hpf.enabled} title="High-pass filter">HPF</span>
    <span class:on={st.gate.enabled} title="Gate">G</span>
    <span class:on={st.comp.enabled} title="Compressor">C</span>
    <span class:on={st.polarity} title="Polarity inverted">INV</span>
  </div>
  <button class="eq" onclick={() => onselect(channel.id)} tabindex="-1" aria-hidden="true">
    <EqCurve eq={st.eq} hpf={st.hpf} width={72} height={18} />
  </button>

  <div class="routes" aria-label="Bus assignment">
    {#each mixer.shape!.buses as bus (bus.id)}
      <span class:on={st.routes[bus.id]} title={bus.name}>{short(bus)}</span>
    {/each}
  </div>

  <div class="pan">
    {#if channel.width <= 2}
      <Pan path={channelPath(channel.id, 'pan', 'lr')} label={`${channel.name} pan`} />
    {:else}
      <span class="fixed" title="Channels map by label">{channel.layout} direct</span>
    {/if}
  </div>

  <div class="keys">
    <button class="key" aria-disabled={mixer.locked} aria-pressed={solo === 'pfl'} onclick={() => toggleSolo('pfl')} aria-label={`${channel.name} PFL`}>PFL</button>
    <button class="key afl" aria-disabled={mixer.locked} aria-pressed={solo === 'afl'} onclick={() => toggleSolo('afl')} aria-label={`${channel.name} AFL`}>AFL</button>
  </div>
  <button
    class="key mute big"
    aria-disabled={mixer.locked}
    aria-pressed={mute}
    class:vca-muted={vcaMuted && !mute}
    onclick={() => mixer.set(channelPath(channel.id, 'mute'), !mute)}
    aria-label={`${channel.name} mute`}
    data-path={channelPath(channel.id, 'mute')}>MUTE</button
  >

  <div class="fm">
    <Fader path={channelPath(channel.id, 'fader')} label={`${channel.name} fader`} />
    <div class="meter"><Meter group="channels" id={channel.id} legs={channel.width} label={`${channel.name} input meter`} /></div>
  </div>

  <div class="foot">
    <output class="num value" data-testid={`fader-value-${channel.id}`}><span>{formatDb(fader)}</span></output>
    <div class="vca">
      {#each vcas as v (v.id)}<span title={`${v.name} offset ${formatDb(mixer.state!.vcas[v.id]!.offset)} dB`}>{v.name.slice(0, 4)}</span>{/each}
      {#if vcas.length > 0 && effective !== fader}<span class="eff num" title="Effective gain">{formatDb(effective)}</span>{/if}
    </div>
  </div>
</section>

<style>
  .strip {
    width: 86px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 4px 6px;
    background: var(--strip);
    border-right: 1px solid var(--line);
    min-height: 0;
  }
  .strip.alt {
    background: var(--strip-alt);
  }
  .strip.selected {
    background: var(--strip-sel);
    box-shadow: inset 0 2px 0 var(--accent);
  }
  .head {
    all: unset;
    display: grid;
    grid-template-columns: auto 1fr;
    grid-template-rows: auto auto;
    column-gap: 4px;
    cursor: pointer;
    padding: 3px 4px;
    border-radius: 3px;
    background: var(--well);
    color: #e6edf3;
    min-height: 34px;
  }
  .head:focus-visible {
    outline: 2px solid var(--focus);
  }
  .no {
    font-size: 10px;
    color: #8b96a4;
  }
  .badge {
    justify-self: end;
    font-size: 9px;
    font-weight: 600;
    color: #0b0d10;
    background: #8b96a4;
    border-radius: 2px;
    padding: 0 3px;
    line-height: 13px;
  }
  .name {
    grid-column: 1 / -1;
    font-weight: 600;
    font-size: 12.5px;
    letter-spacing: 0.02em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .selected .name {
    color: var(--accent);
  }
  :global([data-theme='light']) .selected .name {
    color: #ffb000;
  }
  .afv {
    height: 16px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-align: center;
    border-radius: 2px;
    line-height: 16px;
    color: var(--text-faint);
    border: 1px solid var(--line);
  }
  .afv .none {
    opacity: 0.6;
  }
  .afv.program {
    background: var(--pgm);
    border-color: var(--pgm);
    color: #fff;
  }
  .afv.preview {
    background: color-mix(in srgb, var(--pvw) 25%, transparent);
    border-color: var(--pvw);
    color: var(--pvw);
  }
  .afv.closed:not(.program):not(.preview) {
    text-decoration: line-through;
  }
  .proc,
  .routes {
    display: flex;
    gap: 2px;
  }
  .proc span,
  .routes span {
    flex: 1;
    text-align: center;
    font-size: 9px;
    font-weight: 600;
    line-height: 14px;
    border-radius: 2px;
    color: var(--text-faint);
    background: var(--panel-2);
    overflow: hidden;
    white-space: nowrap;
  }
  .proc span.on {
    color: #00191c;
    background: var(--afl);
  }
  .routes span.on {
    color: var(--accent-ink);
    background: var(--accent);
  }
  .eq {
    all: unset;
    cursor: pointer;
  }
  .pan {
    min-height: 18px;
  }
  .fixed {
    display: block;
    font-size: 9.5px;
    color: var(--text-faint);
    text-align: center;
    line-height: 18px;
  }
  .keys {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
  }
  .big {
    height: 26px;
    font-size: 12px;
  }
  .vca-muted {
    border-color: var(--mute);
    color: var(--mute);
  }
  .fm {
    flex: 1;
    min-height: 140px;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px;
  }
  .meter {
    min-width: 8px;
    padding: 17px 0;
  }
  .foot {
    display: flex;
    flex-direction: column;
    gap: 2px;
    align-items: stretch;
  }
  .value {
    font-size: 11.5px;
    text-align: center;
    background: var(--well);
    border-radius: 2px;
    color: #e6edf3;
    line-height: 18px;
  }
  /* The text changes with every update. A flex item is never a relayout
     boundary, so the text sits in a block child of fixed size with strict
     containment, which lays out nothing outside it. */
  .value span {
    display: block;
    height: 18px;
    contain: strict;
  }
  .vca {
    display: flex;
    gap: 2px;
    min-height: 14px;
    justify-content: center;
    flex-wrap: wrap;
  }
  .vca span {
    font-size: 9px;
    font-weight: 600;
    padding: 0 3px;
    border-radius: 2px;
    background: var(--panel-2);
    color: var(--text-dim);
    line-height: 14px;
  }
  .vca .eff {
    color: var(--accent);
  }
</style>
