<script lang="ts">
  import { fit } from '../lib/format';
  import type { ElementShape } from '../lib/contract/types';
  import { LAYOUT_LABELS } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import type { MeterGroup } from '../lib/meters/store';
  import { pointer } from '../lib/state/pointer';
  import { formatDb } from '../lib/audio/db';
  import { resetLoudness } from '../lib/state/actions';
  import Fader from './Fader.svelte';
  import Meter from './Meter.svelte';
  import Loudness from './Loudness.svelte';
  import TalkKey from './TalkKey.svelte';
  import LayoutBadge from './LayoutBadge.svelte';
  import HoldMute from './HoldMute.svelte';
  import { tip } from '../lib/tip';

  interface Props {
    group: Exclude<MeterGroup, 'channels' | 'monitor'>;
    element: ElementShape;
    tag?: string;
    selected?: boolean;
    onselect?: (id: string) => void;
    /** Keep the height of the talkback row, so masters beside mix-minus masters line up. */
    talkRow?: boolean;
  }
  let { group, element, tag = '', selected = false, onselect, talkRow = false }: Props = $props();
  const mixer = getMixer();
  const path = (key: string) => pointer(group, element.id, key);
  const mute = $derived(mixer.value<boolean>(path('mute')));
  const fader = $derived(mixer.value<number>(path('fader')));
  const labels = $derived(LAYOUT_LABELS[element.layout]);
  const tone = $derived(mixer.state!.tone);
  /** Line-up tone replaces this output's programme. */
  const toned = $derived(tone.on && tone.targets.includes(element.id));
  /**
   * A master of more than two legs grows by LEG_W per leg, so each leg's
   * column under the meter fits its label ("LFE", "Lrs") at 10 px.
   */
  const LEG_W = 18;
  const WIDE_BASE_W = 82;
</script>

<section class="master" class:selected style:width={element.width > 2 ? `${WIDE_BASE_W + element.width * LEG_W}px` : undefined} aria-label={`${element.name} master`} data-master={element.id}>
  <div class="top">
    <button class="head" class:toned onclick={() => onselect?.(element.id)} disabled={!onselect} aria-pressed={selected} use:tip={element.name}>
      <span class="name">{element.name}</span>
      <!-- Every tag keeps its place, shown or not: the name never rewraps.
           Under the kind, the width as a channel strip shows it; line-up
           tone, which replaces the programme, shows in its place. -->
      <span class="tags">
        <span class="tag" class:blank={!tag}>{tag}</span>
        <span class="second">
          <span class="width" class:hidden={toned} data-testid={`layout-${element.id}`}><LayoutBadge layout={element.layout} /></span>
          <span class="tag tone" class:shown={toned} data-testid={`tone-${element.id}`} aria-hidden={!toned}>TONE<span class="sr-only">: line-up tone {tone.level} dBFS replaces this output</span></span>
        </span>
      </span>
    </button>
    <Loudness {group} id={element.id} stereo={element.layout === 'stereo'} onreset={() => resetLoudness(mixer, [element.id])} />
    {#if group === 'mixMinus'}
      <TalkKey id={element.id} name={element.name} />
    {:else if talkRow}
      <div class="talk-space" aria-hidden="true"></div>
    {/if}
    <HoldMute path={path('mute')} name={element.name} />
  </div>
  <Fader path={path('fader')} label={`${element.name} fader`} />
  <div class="meter">
    <Meter {group} id={element.id} legs={element.width} scale="left" label={`${element.name} meter`} />
  </div>
  <div class="foot">
    <!-- Same geometry as the bars the meter layer draws: after the 18 px
         scale, one equal column per leg with a 1 px gap. -->
    <div class="legs" aria-hidden="true" style:grid-template-columns={`repeat(${labels.length}, minmax(0, 1fr))`}>
      {#each labels as l (l)}<span>{l}</span>{/each}
    </div>
    <output class="num value"><span class="fixed">{fit(formatDb(fader), 5)}</span></output>
  </div>
</section>

<style>
  /* Rows: the head and keys above the fader, the fader (at least 124 px)
     and its meter, and the leg labels and value below. The fader takes
     the first column. */
  .master {
    width: 104px;
    flex: none;
    display: grid;
    grid-template-rows: auto minmax(124px, 1fr) auto;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 4px 3px;
    padding: 4px 5px 6px;
    background: var(--panel);
    border-left: 1px solid var(--line);
    min-height: 0;
  }
  .master.selected {
    box-shadow: inset 0 2px 0 var(--on);
  }
  .top {
    grid-column: 1 / -1;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .head {
    all: unset;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 4px;
    background: var(--well);
    color: #e6edf3;
    border-radius: 3px;
    padding: 4px 6px;
    min-height: 34px;
    cursor: pointer;
  }
  .head:disabled {
    cursor: default;
  }
  .head:focus-visible {
    outline: 2px solid var(--focus);
  }
  /* Two lines fit the names of the shape; a longer one is clipped and
     the head carries the full name as its title. */
  .name {
    font-weight: 600;
    font-size: 13px;
    line-height: 14px;
    max-height: 28px;
    overflow: hidden;
    overflow-wrap: anywhere;
    text-align: left;
  }
  .tags {
    flex: none;
    width: 30px;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
  }
  .tag.blank {
    visibility: hidden;
  }
  /* The width and the tone share one cell, so either moves nothing. */
  .second {
    display: grid;
    justify-items: end;
  }
  .second > * {
    grid-area: 1 / 1;
  }
  .width {
    display: flex;
  }
  .width.hidden {
    visibility: hidden;
  }
  .tag.tone {
    visibility: hidden;
    background: var(--tone);
    color: var(--tone-ink);
  }
  .tag.tone.shown {
    visibility: visible;
  }
  .head.toned {
    box-shadow: inset 0 0 0 2px var(--tone);
  }
  .tag {
    white-space: nowrap;
    flex: none;
    line-height: 13px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: #0b0d10;
    background: var(--accent);
    border-radius: 2px;
    padding: 0 3px;
  }
  .talk-space {
    height: 53px;
  }
  .meter {
    padding: 17px 0;
  }
  /* The legs under the meter's column, the value across both. */
  .foot {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    row-gap: 4px;
  }
  .legs {
    grid-column: 2;
    display: grid;
    column-gap: 1px;
    padding-left: 18px;
    font-size: 10px;
    line-height: 13px;
    color: var(--text-faint);
    text-align: center;
  }
  .legs span {
    min-width: 0;
    overflow: visible;
    white-space: nowrap;
  }
  .value {
    grid-column: 1 / -1;
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
</style>
