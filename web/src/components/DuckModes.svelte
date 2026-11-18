<script lang="ts">
  // AUTO / ON / OFF of a duck group. Mode has no owner: any screen sets it
  // while another holds the duck. OFF bypasses the group only after a hold
  // (lib/hold.ts), so no stray touch takes the duck off air; AUTO and ON,
  // and so leaving a bypass, are one deliberate press on these keys.
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
</script>

<div class="modes" role="radiogroup" aria-label={`${name} duck mode`}>
  {#each DUCK_MODES as m (m.id)}
    {#if m.id === 'off'}
      <button
        {@attach pendingWrite(mixer, path)}
        class="key mode bypass"
        role="radio"
        aria-disabled={mixer.locked}
        aria-checked={mode === 'off'}
        aria-label={`${name} ${m.label}${mode === 'off' ? '' : ', hold to bypass'}`}
        title={mode === 'off' ? 'Bypassed: no duck whatever the key' : 'Hold to bypass: no duck whatever the key'}
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
  /* Bypassed: a dashed edge as well as the lit key. */
  .bypass[aria-checked='true'] {
    border-style: dashed;
    border-color: var(--on-ink);
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
