<script lang="ts">
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

  interface Props {
    group: Exclude<MeterGroup, 'channels'>;
    element: ElementShape;
    tag?: string;
    selected?: boolean;
    onselect?: (id: string) => void;
  }
  let { group, element, tag = '', selected = false, onselect }: Props = $props();
  const mixer = getMixer();
  const path = (key: string) => pointer(group, element.id, key);
  const mute = $derived(mixer.value<boolean>(path('mute')));
  const fader = $derived(mixer.value<number>(path('fader')));
  const labels = $derived(LAYOUT_LABELS[element.layout]);
</script>

<section class="master" class:wide={element.width > 2} class:selected aria-label={`${element.name} master`} data-master={element.id}>
  <button class="head" onclick={() => onselect?.(element.id)} disabled={!onselect} aria-pressed={selected}>
    <span class="name">{element.name}</span>
    {#if tag}<span class="tag">{tag}</span>{/if}
  </button>
  <Loudness {group} id={element.id} onreset={() => resetLoudness(mixer, [element.id])} />
  <button class="key mute" aria-disabled={mixer.locked} aria-pressed={mute} onclick={() => mixer.set(path('mute'), !mute)} aria-label={`${element.name} mute`}>MUTE</button>
  <div class="fm">
    <Fader path={path('fader')} label={`${element.name} fader`} />
    <div class="meter">
      <Meter {group} id={element.id} legs={element.width} scale label={`${element.name} meter`} />
    </div>
  </div>
  <div class="legs" style:padding-left="18px">
    {#each labels as l (l)}<span>{l}</span>{/each}
  </div>
  <output class="num value"><span>{formatDb(fader)}</span></output>
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
  .name {
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tag {
    white-space: nowrap;
    flex: none;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: #0b0d10;
    background: var(--accent);
    border-radius: 2px;
    padding: 0 3px;
  }
  .mute {
    height: 26px;
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
