<script lang="ts">
  // AUTO / ON / OFF of a duck group. Mode has no owner: any screen sets it
  // while another holds the duck. OFF bypasses the group only after a hold
  // (lib/hold.ts), so no stray touch takes the duck off air; AUTO and ON,
  // and so leaving a bypass, are one deliberate press on these keys.
  // One tab stop, the checked key: the arrows move the focus among them
  // and select nothing, so walking past OFF or ON changes no live mode.
  import { pendingWrite } from '../lib/pending';
  import { hold } from '../lib/hold';
  import { DUCK_MODES } from '../lib/duck';
  import type { DuckMode } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { duckPath } from '../lib/state/paths';

  /** A bypass takes a hold this long. */
  const BYPASS_HOLD_MS = 500;

  let { id, name }: { id: string; name: string } = $props();
  const mixer = getMixer();
  const path = $derived(duckPath(id, 'mode'));
  const mode = $derived(mixer.value<DuckMode>(path));
  /** The key the arrows moved to, until it loses the focus; else the checked one is the tab stop. */
  let focused = $state<DuckMode | null>(null);
  const stop = (m: DuckMode) => ((focused ?? mode) === m ? 0 : -1);
  function arrow(e: KeyboardEvent, at: number): void {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const next = DUCK_MODES[(at + step + DUCK_MODES.length) % DUCK_MODES.length]!.id;
    focused = next;
    ((e.currentTarget as HTMLElement).parentElement?.querySelector(`[data-mode="${next}"]`) as HTMLElement | null)?.focus();
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="modes" role="radiogroup" aria-label={`${name} duck mode`} onfocusout={(e) => !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null) && (focused = null)}>
  {#each DUCK_MODES as m, i (m.id)}
    {#if m.id === 'off'}
      <button
        {@attach pendingWrite(mixer, path)}
        class="key mode bypass"
        role="radio"
        aria-disabled={mixer.locked}
        aria-checked={mode === 'off'}
        aria-label={`${name} ${m.label}${mode === 'off' ? '' : ', hold to bypass'}`}
        title={mode === 'off' ? 'Bypassed: no duck whatever the key' : 'Hold to bypass: no duck whatever the key'}
        tabindex={stop(m.id)}
        data-mode={m.id}
        onkeydown={(e) => arrow(e, i)}
        use:hold={{ ms: mode === 'off' ? 0 : BYPASS_HOLD_MS, fire: () => mixer.set(path, 'off') }}>{m.key}</button
      >
    {:else}
      <button
        {@attach pendingWrite(mixer, path)}
        class="key mode"
        role="radio"
        aria-disabled={mixer.locked}
        aria-checked={mode === m.id}
        aria-label={`${name} ${m.label}`}
        title={m.label}
        tabindex={stop(m.id)}
        data-mode={m.id}
        onkeydown={(e) => arrow(e, i)}
        onclick={() => mixer.set(path, m.id)}>{m.key}</button
      >
    {/if}
  {/each}
</div>

<style>
  .modes {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 3px;
  }
  .mode {
    padding: 0;
    font-size: 10px;
    letter-spacing: 0.04em;
  }
  .mode {
    height: 24px;
  }
  /* Bypassed: the hatch as well as the lit key. */
  .bypass[aria-checked='true'] {
    background-image: var(--hatch);
  }
  @media (pointer: coarse) {
    .mode {
      height: 44px;
    }
  }
  /* The hold to bypass fills a bar along the bottom of OFF; transform only. */
  .bypass {
    position: relative;
    overflow: hidden;
    touch-action: none;
  }
  .bypass::after {
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
  .bypass:global(.arming)::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
  }
</style>
