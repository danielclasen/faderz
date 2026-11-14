<script lang="ts">
  // Hold-to-talk key of a mix-minus IFB interrupt: pressed (pointer, Space
  // or Enter) it owns and sets interrupt/active, released it clears and
  // releases it. The server also clears it when the session's lease ends.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { mixMinusPath } from '../lib/state/paths';

  interface Props {
    id: string;
    name: string;
  }
  let { id, name }: Props = $props();
  const mixer = getMixer();
  const path = $derived(mixMinusPath(id, 'interrupt', 'active'));
  const sourcePath = $derived(mixMinusPath(id, 'interrupt', 'source'));
  const active = $derived(mixer.value<boolean>(path));
  const source = $derived(mixer.value<string | null>(sourcePath));
  const channels = $derived(mixer.shape!.channels);
  let holding = false;

  function press(): void {
    if (holding || mixer.locked || source === null) return;
    holding = true;
    mixer.touch(path);
    mixer.set(path, true);
  }
  function lift(): void {
    if (!holding) return;
    holding = false;
    mixer.set(path, false);
    mixer.release(path);
  }
  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    press();
  }
  function key(e: KeyboardEvent, on: boolean): void {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    if (on && !e.repeat) press();
    else if (!on) lift();
  }
</script>

<div class="talk">
  <select
    disabled={mixer.locked}
    aria-label={`${name} talkback source`}
    value={source ?? ''}
    onchange={(e) => {
      const v = (e.currentTarget as HTMLSelectElement).value;
      mixer.set(sourcePath, v === '' ? null : v);
    }}
  >
    <option value="">no talkback</option>
    {#each channels as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
  </select>
  <button
    class="key"
    class:on={active}
    aria-disabled={mixer.locked || source === null}
    aria-pressed={active}
    aria-label={`${name} talk, hold`}
    title={source === null ? 'Choose a talkback source first' : 'Hold to talk into this output'}
    onpointerdown={down}
    onpointerup={lift}
    onpointercancel={lift}
    onlostpointercapture={lift}
    onkeydown={(e) => key(e, true)}
    onkeyup={(e) => key(e, false)}
    onblur={lift}
  >TALK</button>
</div>

<style>
  .talk {
    display: grid;
    gap: 3px;
  }
  select {
    width: 100%;
    height: 22px;
    font-size: 11px;
    min-width: 0;
  }
  .key {
    height: 28px;
    touch-action: none;
  }
  .key.on {
    background: var(--talk);
    border-color: var(--talk);
    color: var(--talk-ink);
  }
</style>
