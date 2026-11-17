<script lang="ts">
  import { pendingWrite } from '../lib/pending';
  import { centre, fit } from '../lib/format';
  import type { ChannelShape, ChannelState, Solo, Tap } from '../lib/contract/types';
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
  import { MediaQuery } from 'svelte/reactivity';
  import { pointer } from '../lib/state/pointer';

  interface Props {
    channel: ChannelShape;
    index: number;
    selected: boolean;
    onselect: (id: string, section?: string) => void;
    /** Sends on faders: the fader sets this aux's send level instead of the channel level. */
    sendsTo?: { id: string; name: string } | null;
  }
  let { channel, index, selected, onselect, sendsTo = null }: Props = $props();
  const levelPath = $derived(sendsTo ? channelPath(channel.id, 'sends', sendsTo.id, 'level') : channelPath(channel.id, 'fader'));

  const mixer = getMixer();
  const coarse = new MediaQuery('pointer: coarse');
  const st = $derived(mixer.state!.channels[channel.id] as ChannelState);
  const afv = $derived(mixer.state!.afv[channel.id]);
  const tally = $derived(afv?.source ? mixer.state!.tally[afv.source] : undefined);
  const mute = $derived(mixer.value<boolean>(channelPath(channel.id, 'mute')));
  const solo = $derived(mixer.value<Solo>(channelPath(channel.id, 'solo')));
  const fader = $derived(mixer.value<number>(channelPath(channel.id, 'fader')));
  const tapPath = $derived(channelPath(channel.id, 'meterTap'));
  const meterTap = $derived(mixer.value<Tap>(tapPath));
  const nextTap = $derived<Tap>(meterTap === 'postFader' ? 'preFader' : 'postFader');
  const level = $derived(mixer.value<number>(levelPath));
  const holder = $derived(mixer.ownerOf(levelPath));
  const holderMine = $derived(mixer.isMine(holder));
  /**
   * The strip names the holder in a short form that fits its slot: YOU, a
   * console's name, or the handle tag other screens show for a session.
   * The full label is in the tooltip and the accessible name.
   */
  const holderName = $derived(holder ? (holderMine ? 'YOU' : mixer.ownerShort(holder)) : '');
  const holderText = $derived(holder ? `Fader held by ${mixer.ownerLabel(holder)}` : '');
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
  /** Other sessions with this channel selected: the first one's initial marks the head. */
  const selectors = $derived(mixer.selectedBy(channel.id));
  const selectorsText = $derived(selectors.length > 0 ? `selected on ${selectors.map((o) => mixer.ownerLabel(o)).join(', ')}` : '');
  /** Recall-safe groups: a scene recall leaves these alone (an isolate on a console). */
  const safe = $derived(mixer.state!.recallSafe[channel.id] ?? []);
  const badge = $derived(channel.layout === 'mono' ? '' : channel.layout === 'stereo' ? 'ST' : channel.layout);
  const short = (bus: { name: string; kind: string }) =>
    bus.kind === 'cleanFeed' ? 'CF' : bus.kind === 'me' ? 'ME' : (bus.name.split(' ').at(-1) ?? bus.name).slice(0, 4);

  function toggleSolo(mode: Exclude<Solo, 'off'>): void {
    mixer.set(channelPath(channel.id, 'solo'), solo === mode ? 'off' : mode);
  }
</script>

<section class="strip" class:sof={sendsTo !== null} class:selected class:alt={index % 2 === 1} aria-label={`Channel ${index + 1} ${channel.name}`} data-channel={channel.id}>
  <button
    class="head"
    data-color={st.color}
    onclick={() => onselect(channel.id)}
    aria-pressed={selected}
    use:tip={`${channel.source ? `${channel.name} (${channel.source})` : channel.name}${safe.length > 0 ? `; recall safe: ${safe.join(', ')}` : ''}${selectorsText ? `; ${selectorsText}` : ''}`}
  >
    <span class="no num">{index + 1}</span>
    <span class="name">{channel.name}</span>
    <span class="line2">
      <span class="source">{channel.source ?? ''}</span>
      <span class="badges">
        <!-- First, so appearing moves no other badge; the sessions are in
             the head's tooltip and accessible name. -->
        <span class="badge user" class:shown={selectors.length > 0} aria-hidden="true" data-testid={`selected-by-${channel.id}`}
          >{selectors[0] ? mixer.ownerInitial(selectors[0]) : ''}</span
        >
        <span class="sr-only">{selectorsText ? `, ${selectorsText}` : ''}</span>
        <!-- The groups are in the head's tooltip and accessible name. -->
        <span class="badge iso" class:shown={safe.length > 0} aria-hidden="true" data-testid={`iso-${channel.id}`}>ISO</span>
        <span class="sr-only">{safe.length > 0 ? `, recall safe: ${safe.join(', ')}` : ''}</span>
        <span class="badge alt" class:shown={st.input === 'b'} title="Fed from its B input" aria-hidden={st.input !== 'b'}>B</span>
        {#if badge}<span class="badge">{badge}</span>{/if}
      </span>
    </span>
  </button>

  <!-- Tally in words as well as colour: PGM and PVW name the bus of the
       switcher, CLOSED says the AFV gate holds the channel down. PGM is a
       filled band with a dot, mute a filled key: they differ in shape too. -->
  <div
    class="afv"
    class:program={tally === 'program'}
    class:preview={tally === 'preview'}
    class:closed={afv?.enabled && !afv.open && tally !== 'program'}
    data-testid={`tally-${channel.id}`}
  >
    {#if afv?.enabled && afv.source}
      <span class="fixed" title={`Audio follows video: ${afv.source}, ${tally ?? 'no tally'}, ${afv.open ? 'open' : 'closed'}`}
        >{centre(`${tally === 'program' ? 'PGM' : tally === 'preview' ? 'PVW' : afv.open ? 'AFV' : 'CLOSED'} ${afv.source}`, 12)}</span
      >
    {:else}
      <span class="none">-</span>
    {/if}
  </div>

  <!-- On a touch screen a chip is too narrow for a finger: the row is one
       target that opens the processing panel there. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <div class="proc" role="group" aria-label={`${channel.name} processing`} onclick={() => coarse.current && onselect(channel.id, 'input')}>
    {#each [
      { key: 'HPF', on: st.hpf.enabled, to: 'input', what: 'High-pass filter' },
      { key: 'G', on: st.gate.enabled, to: 'dynamics', what: 'Gate' },
      { key: 'C', on: st.comp.enabled, to: 'dynamics', what: 'Compressor' },
      { key: 'INV', on: st.polarity.some(Boolean), to: 'input', what: 'Polarity inverted on a leg' },
    ] as chip (chip.key)}
      <button
        class="chip"
        tabindex="-1"
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
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <div class="routes" role="group" aria-label={`${channel.name} bus assignment`} onclick={() => coarse.current && onselect(channel.id, 'assign')}>
    {#each mixer.shape!.buses as bus (bus.id)}
      {@const path = pointer('channels', channel.id, 'routes', bus.id)}
      <button {@attach pendingWrite(mixer, path)}
        class="chip"
        tabindex="-1"
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
      <Pan path={channelPath(channel.id, 'pan', 'lr')} label={`${channel.name} pan`} tabindex={-1} />
    {:else}
      <span class="fixed" title="Channels map by label">{channel.layout} direct</span>
    {/if}
  </div>

  <div class="ctl">
    <button {@attach pendingWrite(mixer, channelPath(channel.id, 'solo'))} class="key pfl" aria-disabled={mixer.locked} aria-pressed={solo === 'pfl'} onclick={() => toggleSolo('pfl')} aria-label={`${channel.name} PFL`}>PFL</button>
    <button {@attach pendingWrite(mixer, channelPath(channel.id, 'solo'))} class="key afl" aria-disabled={mixer.locked} aria-pressed={solo === 'afl'} onclick={() => toggleSolo('afl')} aria-label={`${channel.name} AFL`}>AFL</button>
    <button {@attach pendingWrite(mixer, channelPath(channel.id, 'mute'))}
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
    <Fader path={levelPath} label={sendsTo ? `${channel.name} send to ${sendsTo.name}` : `${channel.name} fader`} />
    <div class="meter">
      <Meter group="channels" id={channel.id} legs={channel.width} scale="right" label={`${channel.name} meter, ${TAP_LABEL[meterTap]}`} />
      <!-- The meter tap, in the meter's bottom padding, and over it a key
           that flips the meter between pre- and post-fader. Input is set
           in the processing panel; from there a press goes to post-fader.
           The key has no text of its own, so a tap change moves nothing. -->
      <span class="tap fixed" aria-hidden="true" data-testid={`tap-${channel.id}`}>{fit(TAP_KEY[meterTap], 4)}</span>
      <button {@attach pendingWrite(mixer, tapPath)}
        class="tap-key"
        tabindex="-1"
        aria-disabled={mixer.locked}
        aria-label={`${channel.name} meter tap, ${TAP_LABEL[meterTap]}: switch to ${TAP_LABEL[nextTap]}`}
        title={`Meter ${TAP_LABEL[meterTap]}: switch to ${TAP_LABEL[nextTap]}`}
        onclick={() => mixer.set(tapPath, nextTap)}
        data-path={tapPath}
        data-testid={`tap-key-${channel.id}`}></button
      >
    </div>
  </div>

  <div class="foot">
    <output class="num value" data-testid={`fader-value-${channel.id}`}><span class="fixed">{fit(formatDb(level), 5)}</span></output>
    <!-- Who holds the fader, in words; the slot is there whether held or
         not. It is no tab stop: the held fader's tooltip and value text
         give the keyboard the full label. -->
    <span
      class="holder"
      class:held={holder !== null}
      class:mine={holderMine}
      role="note"
      aria-label={holderText || undefined}
      use:tip={holderText}
      data-testid={`holder-${channel.id}`}><span>{holderName}</span></span
    >
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
  /* Sends on faders: a band in the sends colour says the faders are not channel levels. */
  .strip.sof {
    box-shadow: inset 0 3px 0 var(--meter-loud);
  }
  .strip.sof .value {
    color: var(--meter-loud);
  }
  .strip.alt {
    background: var(--strip-alt);
  }
  .strip.selected {
    background: var(--strip-sel);
    box-shadow: inset 0 2px 0 var(--on);
  }
  /* Scribble strip: a colour band, the number and name on the first
     line, the source and badges on the second. Two fixed lines. */
  .head {
    all: unset;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    grid-template-rows: 19px 14px;
    column-gap: 4px;
    align-items: baseline;
    cursor: pointer;
    padding: 2px 4px 3px;
    border-radius: 3px;
    border-top: 3px solid transparent;
    background: var(--well);
    color: #e6edf3;
  }
  .head[data-color='red'] { border-top-color: #ff5a5f; }
  .head[data-color='orange'] { border-top-color: #ff9d3a; }
  .head[data-color='yellow'] { border-top-color: #f2d53c; }
  .head[data-color='green'] { border-top-color: #3ccf75; }
  .head[data-color='cyan'] { border-top-color: #3fd0e0; }
  .head[data-color='blue'] { border-top-color: #4d8dff; }
  .head[data-color='purple'] { border-top-color: #b07cff; }
  .head:focus-visible {
    outline: 2px solid var(--focus);
  }
  .no {
    font-size: 11px;
    color: #c9d1db;
  }
  .name {
    font-weight: 600;
    font-size: 15px;
    line-height: 19px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .line2 {
    grid-column: 1 / -1;
    display: flex;
    align-items: baseline;
    gap: 3px;
    min-width: 0;
  }
  .source {
    flex: 1;
    font-size: 10px;
    line-height: 14px;
    color: #9aa5b3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .badge {
    font-size: 10px;
    font-weight: 600;
    color: #0b0d10;
    background: #8b96a4;
    border-radius: 2px;
    padding: 0 2px;
    line-height: 13px;
  }
  .badges {
    flex: none;
    display: flex;
    gap: 2px;
  }
  /* Hidden badges give their room to the source. */
  .badge:not(.shown):is(.alt, .iso, .user) {
    display: none;
  }
  /* Another screen's selection: a round mark with its initial. */
  .badge.user {
    min-width: 13px;
    border-radius: 7px;
    text-align: center;
    background: var(--accent);
    color: var(--accent-ink);
  }
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
  .selected .head {
    box-shadow: 0 0 0 2px var(--on);
  }
  .afv {
    height: 16px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-align: center;
    border-radius: 2px;
    color: var(--text-faint);
    border: 1px solid var(--line);
  }
  /* The text changes with the tally; it sits in a block of fixed size
     with strict containment, so it lays out nothing outside it. */
  .afv span {
    display: block;
    height: 14px;
    line-height: 14px;
    contain: strict;
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
  .afv.program {
    position: relative;
  }
  .afv.program::before {
    content: '';
    position: absolute;
    left: 3px;
    top: 4px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #fff;
  }
  .afv.closed:not(.preview) {
    border-style: dashed;
    color: var(--text-dim);
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
    color: var(--afl-ink);
    background: var(--afl);
  }
  .routes .chip.on {
    color: var(--on-ink);
    background: var(--on);
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
    font-size: 10px;
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
    bottom: 2px;
    height: 12px;
    line-height: 12px;
    font-size: 10px;
    text-align: center;
    color: var(--text-faint);
    contain: strict;
  }
  .tap-key {
    all: unset;
    position: absolute;
    left: 0;
    right: 0;
    bottom: 2px;
    height: 12px;
    cursor: pointer;
    touch-action: manipulation;
  }
  .tap-key:hover {
    box-shadow: inset 0 0 0 1px var(--line-strong);
    border-radius: 2px;
  }
  .tap-key:focus-visible {
    outline: 2px solid var(--focus);
  }
  .tap-key[aria-disabled='true'] {
    cursor: default;
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
  .holder {
    height: 14px;
    border-radius: 2px;
    font-size: 10px;
    font-weight: 600;
    text-align: center;
    color: #fff;
  }
  .holder span {
    display: block;
    height: 14px;
    line-height: 14px;
    padding: 0 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    contain: strict;
  }
  .holder.held {
    background: var(--owned);
  }
  .holder.held.mine {
    background: var(--on);
    color: var(--on-ink);
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
    font-size: 10px;
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
  @media (pointer: coarse) {
    .ctl .key {
      height: 36px;
    }
    .chip {
      line-height: 24px;
      pointer-events: none;
    }
    .proc,
    .routes {
      cursor: pointer;
    }
    .eq {
      height: 24px;
      display: flex;
      align-items: center;
    }
    .pan {
      min-height: 28px;
    }
    /* The tap key keeps a finger-sized target below the meter. */
    .meter {
      padding-bottom: 26px;
    }
    .tap {
      bottom: 7px;
    }
    .tap-key {
      bottom: 1px;
      height: 24px;
    }
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
      grid-template-rows: 19px;
    }
    .line2 {
      display: none;
    }
    /* Three equal keys, each at least 24 px wide (WCAG 2.5.8). */
    .ctl {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      gap: 2px;
    }
    .ctl .key {
      padding: 0;
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
