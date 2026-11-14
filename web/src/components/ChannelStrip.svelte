<script lang="ts">
  import { fit } from '../lib/format';
  import type { ChannelShape, ChannelState, Solo } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { channelPath } from '../lib/state/paths';
  import { effectiveGainDb, formatDb } from '../lib/audio/db';
  import Fader from './Fader.svelte';
  import Meter from './Meter.svelte';
  import Pan from './Pan.svelte';
  import EqCurve from './EqCurve.svelte';
  import { TAP_KEY, TAP_LABEL } from '../lib/audio/taps';
  import { tip } from '../lib/tip';
  import { hold } from '../lib/hold';
  import { pointer } from '../lib/state/pointer';

  interface Props {
    channel: ChannelShape;
    index: number;
    selected: boolean;
    onselect: (id: string, section?: string) => void;
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
  const vcaText = $derived(
    vcas.length === 0
      ? ''
      : `${vcas.map((v) => `VCA ${v.name} ${formatDb(mixer.state!.vcas[v.id]!.offset)} dB`).join(', ')}; effective gain ${formatDb(effective)} dB`,
  );
  /** Recall-safe groups: a scene recall leaves these alone (an isolate on a console). */
  const safe = $derived(mixer.state!.recallSafe[channel.id] ?? []);
  const badge = $derived(channel.layout === 'mono' ? '' : channel.layout === 'stereo' ? 'ST' : channel.layout);
  const short = (bus: { name: string; kind: string }) =>
    bus.kind === 'cleanFeed' ? 'CF' : bus.kind === 'me' ? 'ME' : (bus.name.split(' ').at(-1) ?? bus.name).slice(0, 4);

  function toggleSolo(mode: Exclude<Solo, 'off'>): void {
    mixer.set(channelPath(channel.id, 'solo'), solo === mode ? 'off' : mode);
  }
</script>

<section class="strip" class:selected class:alt={index % 2 === 1} aria-label={`Channel ${index + 1} ${channel.name}`} data-channel={channel.id}>
  <button class="head" onclick={() => onselect(channel.id)} aria-pressed={selected} use:tip={channel.name}>
    <span class="no num">{index + 1}</span>
    <span class="badges">
      <span class="badge iso" class:shown={safe.length > 0} title={`Recall safe: ${safe.join(', ')}`} aria-hidden={safe.length === 0} data-testid={`iso-${channel.id}`}>ISO</span>
      <span class="badge alt" class:shown={st.input === 'b'} title="Fed from its B input" aria-hidden={st.input !== 'b'}>B</span>
      {#if badge}<span class="badge">{badge}</span>{/if}
    </span>
    <span class="name">{channel.name}</span>
  </button>

  <div class="afv" class:program={tally === 'program'} class:preview={tally === 'preview'} class:closed={afv?.enabled && !afv.open}>
    {#if afv?.enabled && afv.source}
      <span title={`Audio follows video: ${afv.source} ${tally ?? 'no tally'}`}>AFV {afv.source}</span>
    {:else}
      <span class="none">-</span>
    {/if}
  </div>

  <div class="proc" role="group" aria-label={`${channel.name} processing`}>
    {#each [
      { key: 'HPF', on: st.hpf.enabled, to: 'input', what: 'High-pass filter' },
      { key: 'G', on: st.gate.enabled, to: 'dynamics', what: 'Gate' },
      { key: 'C', on: st.comp.enabled, to: 'dynamics', what: 'Compressor' },
      { key: 'INV', on: st.polarity.some(Boolean), to: 'input', what: 'Polarity inverted on a leg' },
    ] as chip (chip.key)}
      <button
        class="chip"
        class:on={chip.on}
        onclick={() => onselect(channel.id, chip.to)}
        title={`${chip.what} ${chip.on ? 'in' : 'out'}: open it in the processing panel`}
        aria-label={`${channel.name} ${chip.what}, ${chip.on ? 'in' : 'out'}`}>{chip.key}</button
      >
    {/each}
  </div>
  <button class="eq" onclick={() => onselect(channel.id)} tabindex="-1" aria-hidden="true">
    <EqCurve eq={st.eq} hpf={st.hpf} width={72} height={18} />
  </button>

  <!-- Every bus is an output that leaves the mixer: assigning or removing
       a channel takes a hold, as muting an output does. -->
  <div class="routes" role="group" aria-label={`${channel.name} bus assignment`}>
    {#each mixer.shape!.buses as bus (bus.id)}
      {@const path = pointer('channels', channel.id, 'routes', bus.id)}
      <button
        class="chip"
        class:on={st.routes[bus.id]}
        aria-pressed={st.routes[bus.id]}
        aria-disabled={mixer.locked}
        use:hold={{ ms: 500, fire: () => mixer.set(path, !st.routes[bus.id]) }}
        title={`${bus.name}: hold to ${st.routes[bus.id] ? 'remove' : 'assign'}`}
        aria-label={`${channel.name} to ${bus.name}, hold to ${st.routes[bus.id] ? 'remove' : 'assign'}`}
        data-path={path}>{short(bus)}</button
      >
    {/each}
  </div>

  <div class="pan">
    {#if channel.width <= 2}
      <Pan path={channelPath(channel.id, 'pan', 'lr')} label={`${channel.name} pan`} />
    {:else}
      <span class="fixed" title="Channels map by label">{channel.layout} direct</span>
    {/if}
  </div>

  <div class="ctl">
    <button class="key" aria-disabled={mixer.locked} aria-pressed={solo === 'pfl'} onclick={() => toggleSolo('pfl')} aria-label={`${channel.name} PFL`}>PFL</button>
    <button class="key afl" aria-disabled={mixer.locked} aria-pressed={solo === 'afl'} onclick={() => toggleSolo('afl')} aria-label={`${channel.name} AFL`}>AFL</button>
    <button
      class="key mute big"
      aria-disabled={mixer.locked}
      aria-pressed={mute}
      class:vca-muted={vcaMuted && !mute}
      onclick={() => mixer.set(channelPath(channel.id, 'mute'), !mute)}
      aria-label={`${channel.name} mute`}
      data-path={channelPath(channel.id, 'mute')}>MUTE</button
    >
  </div>

  <div class="fm">
    <Fader path={channelPath(channel.id, 'fader')} label={`${channel.name} fader`} />
    <div class="meter">
      <Meter group="channels" id={channel.id} legs={channel.width} scale="right" label={`${channel.name} meter, ${TAP_LABEL[st.meterTap]}`} />
      <!-- The meter tap, in the meter's bottom padding. -->
      <span class="tap fixed" aria-hidden="true" data-testid={`tap-${channel.id}`}>{fit(TAP_KEY[st.meterTap], 4)}</span>
    </div>
  </div>

  <div class="foot">
    <output class="num value" data-testid={`fader-value-${channel.id}`}><span class="fixed">{fit(formatDb(fader), 5)}</span></output>
    <!-- One fixed row, whatever the memberships: the full list and the
         effective gain are in its tooltip and its accessible name. -->
    {#if vcas.length > 0}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div class="vca" role="note" tabindex="0" aria-label={vcaText} use:tip={vcaText}>
        <span class="names">{vcas.map((v) => v.name).join(' ')}</span>
        <span class="eff" class:shown={effective !== fader}><span class="fixed">{fit(formatDb(effective), 5)}</span></span>
      </div>
    {:else}
      <div class="vca" aria-hidden="true"></div>
    {/if}
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
    content-visibility: auto;
    contain-intrinsic-size: auto 86px auto 600px;
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
  .badges {
    justify-self: end;
    display: flex;
    gap: 2px;
  }
  /* Reserved space: shown or not, the B badge never moves the head. */
  .badge.alt,
  .badge.iso {
    visibility: hidden;
    background: var(--listen);
  }
  .badge.iso {
    background: #c9d1db;
  }
  .badge.shown {
    visibility: visible;
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
  /* Chips are buttons: 18 px tall, with a hit area of 24 px. */
  .chip {
    all: unset;
    position: relative;
    flex: 1;
    min-width: 0;
    text-align: center;
    font-size: 10px;
    font-weight: 600;
    line-height: 18px;
    border-radius: 2px;
    color: var(--text-faint);
    background: var(--panel-2);
    white-space: nowrap;
    cursor: pointer;
    touch-action: manipulation;
  }
  .chip::before {
    content: '';
    position: absolute;
    inset: -3px 0;
  }
  .chip:focus-visible {
    outline: 2px solid var(--focus);
  }
  .proc .chip.on {
    color: #00191c;
    background: var(--afl);
  }
  .routes .chip.on {
    color: var(--accent-ink);
    background: var(--accent);
  }
  /* A hold fills a bar along the bottom of a route chip; transform only. */
  .routes .chip::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: var(--text);
    transform: scaleX(0);
    transform-origin: left;
  }
  .routes :global(.chip.arming)::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
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
  /* PFL and AFL side by side, MUTE across both below. */
  .ctl {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px 3px;
  }
  .big {
    grid-column: 1 / -1;
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
    position: relative;
    min-width: 8px;
    padding: 17px 0;
  }
  .tap {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 3px;
    height: 10px;
    line-height: 10px;
    font-size: 7.5px;
    text-align: center;
    color: var(--text-faint);
    contain: strict;
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
    height: 14px;
    border-radius: 2px;
  }
  .vca:focus-visible {
    outline: 2px solid var(--focus);
  }
  .vca > span {
    font-size: 9px;
    font-weight: 600;
    padding: 0 3px;
    border-radius: 2px;
    background: var(--panel-2);
    color: var(--text-dim);
    line-height: 14px;
    height: 14px;
  }
  .vca .names {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Reserved: shown or not, the effective gain never resizes the names.
     Its text changes with every fader move, so it sits in a block child
     of fixed size with strict containment, as the fader value does. */
  .vca .eff {
    flex: none;
    width: 36px;
    box-sizing: border-box;
    font-size: 8.5px;
    color: var(--accent);
    visibility: hidden;
  }
  .vca .eff span {
    display: block;
    height: 14px;
    contain: strict;
  }
  .vca .eff.shown {
    visibility: visible;
  }
  /* Short screens (a phone in landscape): the processing, EQ and route
     rows, which the processing panel shows in full, give their height to
     the fader; the name takes one line and PFL, AFL and MUTE one row. */
  @media (max-height: 500px) {
    .strip {
      gap: 3px;
      padding: 3px 3px 4px;
      contain-intrinsic-size: auto 86px auto 280px;
    }
    .proc,
    .eq,
    .routes,
    .vca {
      display: none;
    }
    .head {
      min-height: 0;
      grid-template-columns: auto auto 1fr;
      grid-template-rows: auto;
      align-items: baseline;
    }
    .name {
      grid-column: auto;
      grid-row: 1;
      grid-column-start: 2;
    }
    .badges {
      grid-column: 3;
    }
    .ctl {
      grid-template-columns: auto auto 1fr;
      gap: 2px;
    }
    .ctl .key {
      padding: 0 2px;
      font-size: 10px;
      letter-spacing: 0;
    }
    .big {
      grid-column: auto;
      height: 22px;
    }
    .fm {
      min-height: 100px;
    }
    .meter {
      padding: 17px 0;
    }
  }
</style>
