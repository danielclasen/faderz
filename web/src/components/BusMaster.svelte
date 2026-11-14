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
  import HoldMute from './HoldMute.svelte';
  import { tip } from '../lib/tip';

  interface Props {
    group: Exclude<MeterGroup, 'channels'>;
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
</script>

<section class="master" class:wide={element.width > 2} class:selected aria-label={`${element.name} master`} data-master={element.id}>
  <button class="head" class:toned onclick={() => onselect?.(element.id)} disabled={!onselect} aria-pressed={selected} use:tip={element.name}>
    <span class="name">{element.name}</span>
    <!-- Both tags keep their place, shown or not: the name never rewraps. -->
    <span class="tags">
      <span class="tag" class:blank={!tag}>{tag}</span>
      <span class="tag tone" class:shown={toned} data-testid={`tone-${element.id}`} aria-hidden={!toned}>TONE<span class="sr-only">: line-up tone {tone.level} dBFS replaces this output</span></span>
    </span>
  </button>
  <Loudness {group} id={element.id} stereo={element.layout === 'stereo'} onreset={() => resetLoudness(mixer, [element.id])} />
  {#if group === 'mixMinus'}
    <TalkKey id={element.id} name={element.name} />
  {:else if talkRow}
    <div class="talk-space" aria-hidden="true"></div>
  {/if}
  <HoldMute path={path('mute')} name={element.name} />
  <div class="fm">
    <Fader path={path('fader')} label={`${element.name} fader`} />
    <div class="meter">
      <Meter {group} id={element.id} legs={element.width} scale label={`${element.name} meter`} />
    </div>
  </div>
  <div class="legs" style:padding-left="18px">
    {#each labels as l (l)}<span>{l}</span>{/each}
  </div>
  <output class="num value"><span class="fixed">{fit(formatDb(fader), 5)}</span></output>
</section>

<style>
  .master {
    width: 104px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 5px 6px;
    background: var(--panel);
    border-left: 1px solid var(--line);
    min-height: 0;
  }
  .master.wide {
    width: 136px;
  }
  .master.selected {
    box-shadow: inset 0 2px 0 var(--accent);
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
    font-size: 9px;
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
  .fm {
    flex: 1;
    min-height: 140px;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px;
  }
  .meter {
    padding: 17px 0;
  }
  .legs {
    display: flex;
    justify-content: space-around;
    font-size: 8.5px;
    color: var(--text-faint);
    margin-left: 52px;
    padding-left: 0 !important;
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
</style>
