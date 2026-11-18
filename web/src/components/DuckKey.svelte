<script lang="ts">
  // DUCK key of a duck group (lib/duck.ts DuckPress): pressed (pointer,
  // Space or Enter) it holds the duck in; a tap shorter than LATCH_MS
  // latches it on, or back to auto. Only a release that ends the press
  // counts as one (pointerup, keyup): a cancelled touch, a lost pointer
  // capture, focus leaving the key, the page going hidden or away and the
  // key leaving the screen end the hold and change no mode.
  // With `shortcut` n, Shift+D+n on the keyboard presses it too, while the
  // digit is down; letting go of Shift or D ends that press.
  import { pendingWrite } from '../lib/pending';
  import { DuckPress } from '../lib/duck';
  import type { DuckMode } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { duckPath } from '../lib/state/paths';

  interface Props {
    id: string;
    name: string;
    /** 1..8: the digit of its Shift+D shortcut. */
    shortcut?: number;
  }
  let { id, name, shortcut }: Props = $props();
  const mixer = getMixer();
  const holdPath = $derived(duckPath(id, 'hold'));
  const modePath = $derived(duckPath(id, 'mode'));
  const hold = $derived(mixer.value<boolean>(holdPath));
  const mode = $derived(mixer.value<DuckMode>(modePath));
  const holder = $derived(mixer.ownerOf(holdPath));
  // svelte-ignore state_referenced_locally
  const key = new DuckPress(mixer, id, () => name);

  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    key.press(performance.now());
  }
  function keyboard(e: KeyboardEvent, on: boolean): void {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    if (on && !e.repeat) key.press(performance.now());
    else if (!on) key.lift(performance.now());
  }

  /** Shift and D are down, for the shortcut. */
  let chord = false;
  /** The shortcut pressed the key: its digit is down. */
  let byShortcut = false;
  const typing = (e: KeyboardEvent) => e.target instanceof HTMLElement && !!e.target.closest('input, select, textarea, [contenteditable="true"]');
  function shortcutDown(e: KeyboardEvent): void {
    if (shortcut === undefined || e.altKey || e.ctrlKey || e.metaKey || typing(e)) return;
    if (e.code === 'KeyD') chord = e.shiftKey;
    if (!chord || !e.shiftKey || e.code !== `Digit${shortcut}`) return;
    e.preventDefault();
    if (e.repeat || byShortcut) return;
    byShortcut = true;
    key.press(performance.now());
  }
  function shortcutUp(e: KeyboardEvent): void {
    if (e.code === 'KeyD' || e.key === 'Shift') chord = false;
    if (!byShortcut || (e.code !== `Digit${shortcut}` && e.code !== 'KeyD' && e.key !== 'Shift')) return;
    byShortcut = false;
    key.lift(performance.now());
  }
  function blurred(): void {
    chord = false;
    byShortcut = false;
    key.abort();
  }
  // The key leaves the screen: a hold ends with it. A teardown reads state
  // as it was before the update that unmounts the key, so the hold is
  // ended once that update is done (DuckPress checks the shape).
  $effect(() => () => queueMicrotask(() => key.abort()));
</script>

<svelte:window onkeydown={shortcutDown} onkeyup={shortcutUp} onblur={blurred} onpagehide={() => key.abort()} />
<svelte:document onvisibilitychange={() => document.visibilityState === 'hidden' && key.abort()} />

<button
  {@attach pendingWrite(mixer, [modePath, holdPath])}
  class="key duck"
  class:on={hold || mode === 'on'}
  class:latched={mode === 'on'}
  aria-disabled={mixer.locked}
  aria-pressed={hold || mode === 'on'}
  aria-label={`${name} duck${mode === 'on' ? ', latched' : ''}${hold ? `, held by ${mixer.ownerLabel(holder)}` : ''}${shortcut ? `, Shift+D+${shortcut}` : ''}`}
  title={`Hold to duck ${name}; tap to latch it on, tap again for auto${shortcut ? `; keyboard Shift+D+${shortcut}` : ''}`}
  onpointerdown={down}
  onpointerup={() => key.lift(performance.now())}
  onpointercancel={() => key.abort()}
  onlostpointercapture={() => key.abort()}
  onkeydown={(e) => keyboard(e, true)}
  onkeyup={(e) => keyboard(e, false)}
  onblur={() => key.abort()}
  data-testid={`duck-key-${id}`}>DUCK</button
>

<style>
  .duck {
    width: 100%;
    height: 30px;
    font-size: 12px;
    touch-action: none;
  }
  .duck.on {
    background: var(--duck);
    border-color: var(--duck);
    color: var(--duck-ink);
  }
  /* Latched: the key stays lit with a ring, so it reads as left on. */
  .duck.latched {
    box-shadow: 0 0 0 2px var(--duck-ink), 0 0 0 4px var(--duck);
  }
  @media (pointer: coarse) {
    .duck {
      height: 36px;
    }
  }
</style>
