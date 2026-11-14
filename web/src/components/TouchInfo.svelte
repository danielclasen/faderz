<script lang="ts">
  // Touch screens have no hover: a finger resting 600 ms on anything with
  // a title shows that title in a bubble over the view until the next
  // touch, and the lift that ends that rest presses nothing. Controls that
  // act on a hold or a drag (sliders, hold keys, TALK) are left alone, and
  // so is a label with a tooltip of its own (lib/tip.ts), which a long
  // touch already shows.
  const LONG_PRESS_MS = 600;
  let info = $state<{ text: string; x: number; y: number } | null>(null);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let start: { x: number; y: number } | null = null;
  /** A long touch showed a title: the click its lift makes reads it, it does not press. */
  let read = false;

  function cancel(): void {
    if (timer) clearTimeout(timer);
    timer = null;
    start = null;
  }
  function down(e: PointerEvent): void {
    info = null;
    read = false;
    cancel();
    if (e.pointerType !== 'touch') return;
    const el = (e.target as Element | null)?.closest<HTMLElement>('[title]');
    if (!el || el.closest('[role="slider"], .protected, .lock, .reset, .routes, .talk, aside.mock, [data-tip]')) return;
    const text = el.title;
    start = { x: e.clientX, y: e.clientY };
    timer = setTimeout(() => {
      timer = null;
      read = true;
      info = { text, x: Math.min(start!.x, innerWidth - 260), y: Math.max(8, start!.y - 60) };
    }, LONG_PRESS_MS);
  }
  function click(e: MouseEvent): void {
    if (!read) return;
    read = false;
    e.preventDefault();
    e.stopImmediatePropagation();
  }
  function move(e: PointerEvent): void {
    if (start && Math.hypot(e.clientX - start.x, e.clientY - start.y) > 8) cancel();
  }
</script>

<svelte:window onclickcapture={click} onpointerdown={down} onpointermove={move} onpointerup={() => timer && cancel()} onpointercancel={cancel} />

{#if info}
  <div class="info" role="tooltip" data-testid="touch-info" style:left={`${Math.max(8, info.x)}px`} style:top={`${info.y}px`}>{info.text}</div>
{/if}

<style>
  .info {
    position: fixed;
    z-index: 60;
    max-width: 250px;
    padding: 6px 8px;
    border-radius: 3px;
    background: var(--panel);
    border: 1px solid var(--key-edge);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.45);
    color: var(--text);
    font-size: 12px;
    pointer-events: none;
  }
</style>
