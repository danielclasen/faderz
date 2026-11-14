<script lang="ts">
  import type { VcaShape } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { vcaPath } from '../lib/state/paths';
  import { formatDb } from '../lib/audio/db';
  import Fader from './Fader.svelte';

  let { vca }: { vca: VcaShape } = $props();
  const mixer = getMixer();
  const mute = $derived(mixer.value<boolean>(vcaPath(vca.id, 'mute')));
  const offset = $derived(mixer.value<number>(vcaPath(vca.id, 'offset')));
  const members = $derived(mixer.state!.vcas[vca.id]!.members);
</script>

<section class="vca" aria-label={`VCA ${vca.name}`}>
  <div class="head">
    <span class="kind">VCA</span>
    <span class="name">{vca.name}</span>
  </div>
  <div class="members" title={members.map((m) => mixer.shape?.channels.find((c) => c.id === m)?.name ?? m).join(', ')}>
    {members.length} ch
  </div>
  <button class="key mute" aria-disabled={mixer.locked} aria-pressed={mute} onclick={() => mixer.set(vcaPath(vca.id, 'mute'), !mute)} aria-label={`VCA ${vca.name} mute`}>MUTE</button>
  <div class="f"><Fader path={vcaPath(vca.id, 'offset')} label={`VCA ${vca.name} offset`} /></div>
  <output class="num value"><span>{formatDb(offset)}</span></output>
</section>

<style>
  .vca {
    width: 58px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 4px 6px;
    background: var(--panel-2);
    border-left: 1px solid var(--line);
  }
  .head {
    background: var(--well);
    border-radius: 3px;
    padding: 3px 4px;
    min-height: 34px;
    color: #e6edf3;
  }
  .kind {
    display: block;
    font-size: 9px;
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
    font-size: 10.5px;
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
