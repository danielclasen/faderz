<script lang="ts">
  // Talk key of a mix-minus IFB interrupt. Pressed (pointer, Space or
  // Enter) it owns and sets interrupt/active. Held longer than LATCH_MS it
  // is momentary: released, it clears and releases it. A shorter tap
  // latches it on until the next press. Only a release that ends the
  // press latches (pointerup, keyup): a cancelled touch, a lost pointer
  // capture or focus leaving the key ends the talk, so an interrupted
  // press never leaves a presenter's feed replaced. The page going hidden
  // or away ends a held or latched talk: a mouse hold over a tab or app
  // switch gets no blur where a click does not focus the key (Safari).
  // Back on the page, a notice says the talk ended and why: the operator
  // left it on and finds it off.
  // A press on a lit key stops the talk, whichever client lit it. A held
  // or latched talk ends when the key leaves the screen (another view),
  // so no talk outlives the key that stops it. The server also clears it
  // when the owning session releases it or its lease ends.
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
  /**
   * Writes locked, or the serving replica cannot commit: no talk starts,
   * and a stop sends only the release, so the key never shows a talk
   * ended before the state says it is.
   */
  const frozen = $derived(mixer.locked || mixer.notCommitting);

  function off(): void {
    if (!mixer.notCommitting) mixer.set(path, false);
    mixer.release(path);
  }
  function press(): void {
    if (holding) return;
    if (latched) {
      // This press ends the latch; its release does nothing more. While
      // writes are locked or nothing commits the set is not sent but the
      // release is, and
      // the server ends an interrupt its owner releases: the key keeps
      // its latch until the state says the talk is off, so the screen
      // never shows a talk ended before it is.
      off();
      if (!frozen) latched = false;
      return;
    }
    if (active) {
      // Lit by another client, a script or this session without a latch
      // here: this press stops it, and releases it when this session owns
      // it. One another session owns can only be stopped there (the
      // server refuses the write), so say who holds it instead. The
      // release is defensive: this key holds the path only while it holds
      // or latches the talk, so here it only ever sends the set.
      const owner = mixer.ownerOf(path);
      if (owner && !mixer.isMine(owner)) mixer.notify('conflict', `${name} talk is held by ${mixer.ownerLabel(owner)}`, path);
      else if (owner) off();
      else if (!mixer.notCommitting) mixer.set(path, false);
      return;
    }
    if (frozen || source === null) return;
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
  /** Why a talk ended while the page was away, for the notice on return. */
  let endedAway: string | null = null;
  /**
   * The page is hidden or left: a held or latched talk ends. A browser
   * fires visibilitychange before pagehide, so the later, more precise
   * reason replaces the earlier one of the same absence.
   */
  function away(why: string): void {
    if (holding) abort();
    else if (latched) {
      off();
      if (!frozen) latched = false;
    } else if (endedAway === null) return;
    endedAway = why;
  }
  /** The page is back: say once that a talk ended while it was away. */
  function back(): void {
    if (endedAway === null) return;
    mixer.notify('info', `${name} talk ended: ${endedAway}`, path);
    endedAway = null;
  }
  function visibility(): void {
    if (document.visibilityState === 'hidden') away('the page was hidden');
    else back();
  }
  // The key leaves the screen: a held or latched talk ends with it. A
  // layout change that removed the mix-minus left no path to set, so the
  // talk is only released. A teardown reads state as it was before the
  // update that unmounts the key, so the shape is read once it is done.
  $effect(() => () => {
    if (holding || latched) {
      const [p, mm] = [path, id];
      queueMicrotask(() => {
        if (mixer.shape?.mixMinus.some((m) => m.id === mm)) mixer.set(p, false);
        mixer.release(p);
      });
    }
    holding = false;
  });
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

<svelte:window onpagehide={() => away('the page was left')} onpageshow={back} />
<svelte:document onvisibilitychange={visibility} />

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
    aria-disabled={frozen || source === null}
    aria-pressed={active}
    aria-label={`${name} talk${latched ? ', latched' : ''}`}
    title={source === null ? 'Choose a talkback source first' : 'Hold to talk into this output; tap to latch; a tap on a lit key stops the talk'}
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
