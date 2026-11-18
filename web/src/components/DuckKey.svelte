<script lang="ts">
  // DUCK key of a duck group (lib/duck.ts DuckPress): pressed (pointer,
  // Space or Enter) it holds the duck in; a tap shorter than LATCH_MS
  // latches it on, or back to auto. Only a release that ends the press
  // counts as one (pointerup, keyup): a cancelled touch, a lost pointer
  // capture, focus leaving the key, the page going hidden or away and the
  // key leaving the screen end the hold and change no mode. While the
  // group is held its face names the holder instead of DUCK.
  // `shortcut` n names the Shift+D+n shortcut of the group
  // (DuckShortcuts.svelte) in the key's label.
  // `pill` is the compact form in the bank bar: the group's name and
  // what it does, on one key that presses the same way.
  import { pendingWrite } from '../lib/pending';
  import { centre } from '../lib/format';
  import { DuckPress } from '../lib/duck';
  import type { DuckMode } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { duckPath } from '../lib/state/paths';

  interface Props {
    id: string;
    name: string;
    /** 1..8: the digit of its Shift+D shortcut. */
    shortcut?: number;
    pill?: boolean;
  }
  let { id, name, shortcut, pill = false }: Props = $props();
  const mixer = getMixer();
  const holdPath = $derived(duckPath(id, 'hold'));
  const modePath = $derived(duckPath(id, 'mode'));
  const hold = $derived(mixer.value<boolean>(holdPath));
  const mode = $derived(mixer.value<DuckMode>(modePath));
  const holder = $derived(hold ? mixer.ownerOf(holdPath) : null);
  /** Characters of the face: its text changes with the hold, so it keeps one width (lib/format.ts). */
  const FACE = 12;
  /** What the face says: the holder while held, else the key's own word, cut to FACE. */
  const face = $derived.by(() => {
    const t = hold ? (holder ? mixer.ownerShort(holder) : 'HELD') : 'DUCK';
    return centre(t.length > FACE ? `${t.slice(0, FACE - 3)}...` : t, FACE);
  });
  /** What a pill says the group does, in four characters at most. */
  const state = $derived(hold ? 'HELD' : mode === 'on' ? 'ON' : mode === 'off' ? 'BYP' : 'AUTO');
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

  // The key leaves the screen: a hold ends with it. A teardown reads state
  // as it was before the update that unmounts the key, so the hold is
  // ended once that update is done (DuckPress checks the shape).
  $effect(() => () => queueMicrotask(() => key.abort()));
</script>

<svelte:window onpagehide={() => key.abort()} />
<svelte:document onvisibilitychange={() => document.visibilityState === 'hidden' && key.abort()} />

<button
  {@attach pendingWrite(mixer, [modePath, holdPath])}
  class="key duck"
  class:pill
  class:on={hold || mode === 'on'}
  class:latched={mode === 'on'}
  class:bypass={mode === 'off' && !hold}
  aria-disabled={mixer.locked}
  aria-pressed={hold || mode === 'on'}
  aria-label={`${name} duck${mode === 'on' ? ', latched' : ''}${mode === 'off' ? ', bypassed' : ''}${hold ? `, held by ${mixer.ownerLabel(holder)}` : ''}${shortcut ? `, Shift+D+${shortcut}` : ''}`}
  title={`Hold to duck ${name}; tap to latch it on, tap again for auto${mode === 'off' ? '; bypassed: a hold of 0.3 s ducks it' : ''}${shortcut ? `; keyboard Shift+D+${shortcut} holds` : ''}`}
  onpointerdown={down}
  onpointerup={() => key.lift(performance.now())}
  onpointercancel={() => key.abort()}
  onlostpointercapture={() => key.abort()}
  onkeydown={(e) => keyboard(e, true)}
  onkeyup={(e) => keyboard(e, false)}
  onblur={() => key.abort()}
  data-testid={`duck-${pill ? 'pill' : 'key'}-${id}`}
  >{#if pill}<span class="pname">{name}</span><span class="pstate fixed">{state.padEnd(4)}</span>{:else}<span class="face fixed">{face}</span>{/if}</button
>

<style>
  .duck {
    width: 100%;
    height: 30px;
    font-size: 12px;
    touch-action: none;
  }
  /* The holder's name or DUCK, cut to the key with its full text in the
     accessible name. The text changes with the hold: it sits in a block
     of fixed size with strict containment, which lays out nothing outside it. */
  .face {
    display: block;
    height: 16px;
    line-height: 16px;
    contain: strict;
    overflow: hidden;
    font-size: 12px;
    color: inherit;
    /* A handle tag is case-sensitive. */
    text-transform: none;
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
  .duck.pill {
    width: 112px;
    flex: none;
    height: 24px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 4px;
    align-items: center;
    padding: 0 5px;
    font-size: 10px;
    text-align: left;
  }
  .duck.pill.latched {
    box-shadow: inset 0 0 0 1px var(--duck-ink);
  }
  .pname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-transform: none;
    letter-spacing: 0;
    font-size: 11px;
  }
  /* Four characters of the value font, with none of the key's letter
     spacing, so every state word fits its box whole. */
  .pstate {
    display: block;
    width: 4ch;
    letter-spacing: 0;
    height: 14px;
    line-height: 14px;
    contain: strict;
    font-size: 10px;
    color: inherit;
  }
  /* Bypassed: a hatch as well as the word, so it reads from across the desk without colour. */
  .duck.bypass {
    background-image: var(--hatch);
    color: var(--text);
  }
  @media (pointer: coarse) {
    .duck {
      height: 44px;
    }
    .duck.pill {
      height: 32px;
    }
  }
</style>
