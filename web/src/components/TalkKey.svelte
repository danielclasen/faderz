<script lang="ts">
  // Talk key of a mix-minus IFB interrupt. Pressed (pointer, Space or
  // Enter) it owns and sets interrupt/active. Held longer than LATCH_MS it
  // is momentary: released, it clears and releases it. A shorter tap
  // latches it on until the next press. Only a release that ends the
  // press latches (pointerup, keyup): a cancelled touch, a lost pointer
  // capture or focus leaving the key ends the talk, so an interrupted
  // press never leaves a presenter's feed replaced. The server also
  // clears it when the session's lease ends.
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
  /** A press shorter than this latches; a longer one is momentary. */
  const LATCH_MS = 300;
  let holding = false;
  let pressedAt = 0;
  let latched = $state(false);

  function off(): void {
    mixer.set(path, false);
    mixer.release(path);
  }
  function press(): void {
    if (holding) return;
    if (latched) {
      // This press ends the latch; its release does nothing more.
      latched = false;
      holding = false;
      off();
      return;
    }
    if (mixer.locked || source === null) return;
    holding = true;
    pressedAt = performance.now();
    mixer.touch(path);
    mixer.set(path, true);
  }
  /** The press ended where it was made: a short one latches. */
  function lift(): void {
    if (!holding) return;
    holding = false;
    if (performance.now() - pressedAt < LATCH_MS) {
      latched = true;
      return;
    }
    off();
  }
  /** The press was interrupted (touch cancelled, capture lost, focus gone): the talk ends. */
  function abort(): void {
    if (!holding) return;
    holding = false;
    off();
  }
  // Another client or the lease ended the talk: drop the latch with it.
  $effect(() => {
    if (!active && latched && !holding) {
      latched = false;
      mixer.release(path);
    }
  });
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
    class:latched
    aria-disabled={mixer.locked || source === null}
    aria-pressed={active}
    aria-label={`${name} talk${latched ? ', latched' : ''}`}
    title={source === null ? 'Choose a talkback source first' : 'Hold to talk into this output; tap to latch, tap again to stop'}
    onpointerdown={down}
    onpointerup={lift}
    onpointercancel={abort}
    onlostpointercapture={abort}
    onkeydown={(e) => key(e, true)}
    onkeyup={(e) => key(e, false)}
    onblur={abort}
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
  /* Latched: the key stays lit with a ring, so it reads as left on. */
  .key.latched {
    box-shadow: 0 0 0 2px var(--talk-ink), 0 0 0 4px var(--talk);
  }
</style>
