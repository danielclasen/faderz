<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  const mixer = getMixer();

  $effect(() => {
    const timers = mixer.notices.map((n) => setTimeout(() => mixer.dismiss(n.id), n.kind === 'error' ? 6000 : 3500));
    return () => timers.forEach(clearTimeout);
  });
</script>

<div class="toasts" role="log" aria-live="assertive">
  {#each mixer.notices as n (n.id)}
    <div class="toast {n.kind}" data-testid="notice">
      <span>{n.text}</span>
      <button onclick={() => mixer.dismiss(n.id)} aria-label="Dismiss">x</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 34px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    z-index: 20;
    max-width: min(420px, calc(100vw - 32px));
  }
  .toast {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 10px;
    border-radius: 4px;
    background: var(--panel-2);
    border: 1px solid var(--line-strong);
    border-left: 4px solid var(--afl);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.4);
    font-size: 12px;
  }
  .toast.conflict {
    border-left-color: var(--owned);
  }
  .toast.error {
    border-left-color: var(--err);
  }
  .toast span {
    flex: 1;
  }
  button {
    all: unset;
    cursor: pointer;
    color: var(--text-faint);
    padding: 0 4px;
  }
</style>
