<script lang="ts">
  // Mute key of an output that leaves the mixer. Muting takes a hold of
  // HOLD_MS (pointer, Space or Enter) while a bar fills; letting go
  // earlier cancels. Unmuting is one press. Channel mutes stay one press,
  // as on a console.
  import { getMixer } from '../lib/state/mixer.svelte';

  const HOLD_MS = 500;

  interface Props {
    path: string;
    name: string;
  }
  let { path, name }: Props = $props();
  const mixer = getMixer();
  const muted = $derived(mixer.value<boolean>(path));
  let arming = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  function press(): void {
    if (mixer.locked) return;
    if (muted) {
      mixer.set(path, false);
      return;
    }
    if (timer) return;
    arming = true;
    timer = setTimeout(() => {
      timer = null;
      arming = false;
      mixer.set(path, true);
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
  class="key mute protected"
  class:arming
  style:--hold={`${HOLD_MS}ms`}
  aria-disabled={mixer.locked}
  aria-pressed={muted}
  aria-label={`${name} mute, hold to mute`}
  title="Output leaving the mixer: hold to mute, press to unmute"
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
  onblur={lift}><span class="lock" aria-hidden="true"></span>MUTE</button
>

<style>
  .protected {
    position: relative;
    height: 26px;
    touch-action: none;
  }
  /* A small padlock: the key is protected. */
  .lock {
    position: absolute;
    left: 5px;
    top: 50%;
    width: 7px;
    height: 6px;
    margin-top: -1px;
    border-radius: 1px;
    background: currentColor;
  }
  .lock::before {
    content: '';
    position: absolute;
    left: 1px;
    bottom: 5px;
    width: 3px;
    height: 3px;
    border: 1px solid currentColor;
    border-bottom: none;
    border-radius: 3px 3px 0 0;
  }
  /* The hold fills a bar along the bottom of the key; transform only. */
  .protected::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--mute);
    transform: scaleX(0);
    transform-origin: left;
  }
  .protected.arming::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
  }
</style>
