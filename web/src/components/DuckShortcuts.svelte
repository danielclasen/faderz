<script lang="ts">
  // Shift+D+n on the keyboard holds the n-th duck group while the digit is
  // down, in every view, and never latches: a slip of three keys leaves
  // nothing on. Letting go of Shift or D, the window losing the focus or
  // the page going hidden or away ends the press. It is one more key of
  // the group's hold (lib/duck.ts DuckPress): the hold ends when the last
  // key holding it lets go. Not while typing in a field.
  import { DuckPress } from '../lib/duck';
  import { getMixer } from '../lib/state/mixer.svelte';

  const mixer = getMixer();
  const groups = $derived(mixer.shape?.ducks ?? []);
  /** One press per group, by id, for the groups of the current shape. */
  const presses = $derived(new Map(groups.map((g) => [g.id, new DuckPress(mixer, g.id, () => g.name)])));
  /** Shift and D are down. */
  let chord = false;
  /** The group whose digit is down, by its press. */
  let down: { digit: string; press: DuckPress } | null = null;

  const typing = (e: KeyboardEvent) => e.target instanceof HTMLElement && !!e.target.closest('input, select, textarea, [contenteditable="true"]');
  function keydown(e: KeyboardEvent): void {
    if (e.altKey || e.ctrlKey || e.metaKey || typing(e)) return;
    if (e.code === 'KeyD') chord = e.shiftKey;
    const n = /^Digit([1-8])$/.exec(e.code)?.[1];
    if (!chord || !e.shiftKey || !n) return;
    const group = groups[Number(n) - 1];
    if (!group) return;
    e.preventDefault();
    if (e.repeat || down) return;
    down = { digit: e.code, press: presses.get(group.id)! };
    down.press.press(performance.now());
  }
  function keyup(e: KeyboardEvent): void {
    if (e.code === 'KeyD' || e.key === 'Shift') chord = false;
    if (!down || (e.code !== down.digit && e.code !== 'KeyD' && e.key !== 'Shift')) return;
    const p = down.press;
    down = null;
    p.lift(performance.now(), false);
  }
  function abort(): void {
    chord = false;
    const p = down?.press;
    down = null;
    p?.abort();
  }
  // A shape without the group, or this component leaving: the press ends.
  $effect(() => {
    void presses;
    return () => queueMicrotask(abort);
  });
</script>

<svelte:window onkeydown={keydown} onkeyup={keyup} onblur={abort} onpagehide={abort} />
<svelte:document onvisibilitychange={() => document.visibilityState === 'hidden' && abort()} />
