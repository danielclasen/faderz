<script lang="ts">
  import { pendingWrite } from '../lib/pending';
  import { fit } from '../lib/format';
  import type { Solo, VcaShape } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { vcaPath } from '../lib/state/paths';
  import { formatDb } from '../lib/audio/db';
  import Fader from './Fader.svelte';
  import { tip } from '../lib/tip';

  let { vca }: { vca: VcaShape } = $props();
  const mixer = getMixer();
  const mute = $derived(mixer.value<boolean>(vcaPath(vca.id, 'mute')));
  const offset = $derived(mixer.value<number>(vcaPath(vca.id, 'offset')));
  const members = $derived(mixer.state!.vcas[vca.id]!.members);
  const memberNames = $derived(members.map((m) => mixer.shape?.channels.find((c) => c.id === m)?.name ?? m).join(', '));
  const solo = $derived(mixer.value<Solo>(vcaPath(vca.id, 'solo')));
  function toggleSolo(mode: Exclude<Solo, 'off'>): void {
    mixer.set(vcaPath(vca.id, 'solo'), solo === mode ? 'off' : mode);
  }
</script>

<section class="vca" aria-label={`VCA ${vca.name}`}>
  <!-- A press spills the VCA: the strip area shows only its members. A
       button, so the tooltip with the full name and the members reaches
       the keyboard too. -->
  <button
    class="head"
    onclick={() => document.dispatchEvent(new CustomEvent('faderz:spill', { detail: vca.id }))}
    aria-label={`Spill VCA ${vca.name}: show only its members`}
    use:tip={`VCA ${vca.name}: ${memberNames || 'no members'}. Press to show only its members on the strips.`}
  >
    <span class="kind">VCA</span>
    <span class="name">{vca.name}</span>
  </button>
  <div class="members">
    {members.length} ch
  </div>
  <button {@attach pendingWrite(mixer, vcaPath(vca.id, 'solo'))} class="key pfl" aria-disabled={mixer.locked} aria-pressed={solo === 'pfl'} onclick={() => toggleSolo('pfl')} aria-label={`VCA ${vca.name} PFL, all members`}>PFL</button>
  <button {@attach pendingWrite(mixer, vcaPath(vca.id, 'solo'))} class="key afl" aria-disabled={mixer.locked} aria-pressed={solo === 'afl'} onclick={() => toggleSolo('afl')} aria-label={`VCA ${vca.name} AFL, all members`}>AFL</button>
  <button {@attach pendingWrite(mixer, vcaPath(vca.id, 'mute'))} class="key mute" aria-disabled={mixer.locked} aria-pressed={mute} onclick={() => mixer.set(vcaPath(vca.id, 'mute'), !mute)} aria-label={`VCA ${vca.name} mute`}>MUTE</button>
  <div class="f"><Fader path={vcaPath(vca.id, 'offset')} label={`VCA ${vca.name} offset`} /></div>
  <output class="num value"><span class="fixed">{fit(formatDb(offset), 5)}</span></output>
</section>

<style>
  /* 52 px of fader (scale and track) inside the padding and border. */
  .vca {
    width: 62px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 4px 6px;
    background: var(--panel-2);
    border-left: 1px solid var(--line);
  }
  .head {
    all: unset;
    box-sizing: border-box;
    display: block;
    cursor: pointer;
    background: var(--well);
    border-radius: 3px;
    padding: 3px 4px;
    min-height: 34px;
    color: #e6edf3;
  }
  .kind {
    display: block;
    font-size: 10px;
    color: var(--owned);
    font-weight: 600;
    letter-spacing: 0.1em;
  }
  .name {
    font-size: 11px;
    font-weight: 600;
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .members {
    font-size: 10px;
    color: var(--text-dim);
    text-align: center;
    line-height: 16px;
  }
  .mute {
    height: 26px;
    padding: 0 2px;
  }
  .f {
    flex: 1;
    min-height: 140px;
  }
  .value {
    font-size: 11px;
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
