<script lang="ts">
  // Desk lock: the screen keeps showing everything and changes nothing.
  // Locking and unlocking both take a hold of HOLD_MS (pointer, Space or
  // Enter), so neither happens by a stray press.
  import { getMixer } from '../lib/state/mixer.svelte';

  const HOLD_MS = 500;
  const mixer = getMixer();
  let arming = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  function press(): void {
    if (timer) return;
    arming = true;
    timer = setTimeout(() => {
      timer = null;
      arming = false;
      mixer.deskLock = !mixer.deskLock;
    }, HOLD_MS);
  }
  function lift(): void {
    if (timer) clearTimeout(timer);
    timer = null;
    arming = false;
  }
  function key(e: KeyboardEvent, down: boolean): void {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    if (down && !e.repeat) press();
    else if (!down) lift();
  }
</script>

<button
  class="key lock"
  class:arming
  class:on={mixer.deskLock}
  style:--hold={`${HOLD_MS}ms`}
  aria-pressed={mixer.deskLock}
  aria-label={mixer.deskLock ? 'Desk locked, hold to unlock' : 'Lock the desk, hold to lock'}
  title={mixer.deskLock ? 'Desk locked by this screen: it changes nothing. Hold to unlock.' : 'Hold to lock this screen against changes'}
  data-testid="desk-lock"
  onpointerdown={(e) => {
    if (e.button === 0) {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      press();
    }
  }}
  onpointerup={lift}
  onpointercancel={lift}
  onlostpointercapture={lift}
  onkeydown={(e) => key(e, true)}
  onkeyup={(e) => key(e, false)}
  onblur={lift}><span class="fixed label">{mixer.deskLock ? 'LOCKED' : 'LOCK DESK'}</span></button
>

<style>
  .lock {
    position: relative;
    flex: none;
    touch-action: none;
  }
  /* One width for both texts, so a change moves nothing. "LOCK DESK"
     tells it apart from the red LOCK of a link that sends no commands. */
  .label {
    display: inline-block;
    width: 9ch;
    text-align: center;
  }
  /* Phones: the header holds the connection state and the tone lamp; a
     LOCK DESK key would make it wider than the screen. A locked desk
     shows its key in any case, in the room of the theme key (App), so
     a lock made in landscape can be undone in portrait. */
  @media (max-width: 700px) {
    .lock:not(.on) {
      display: none;
    }
    .lock {
      padding: 0 2px;
      letter-spacing: 0;
    }
    .label {
      width: 6ch;
    }
  }
  .lock.on {
    background: var(--text);
    border-color: var(--text);
    color: var(--bg);
  }
  .lock::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--text);
    transform: scaleX(0);
    transform-origin: left;
  }
  .lock.on::after {
    background: var(--bg);
  }
  .lock.arming::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
  }
</style>
