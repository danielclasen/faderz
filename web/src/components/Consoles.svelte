<script lang="ts">
  import { fit } from '../lib/format';
  import { getMixer } from '../lib/state/mixer.svelte';
  const mixer = getMixer();
  const consoles = $derived(mixer.state?.consoles ?? []);
  const channels = $derived(mixer.shape?.channels.length ?? 0);
</script>

<div class="consoles" aria-label="Control surfaces">
  {#each consoles as c (c.id)}
    <span
      class="c"
      class:up={c.connected}
      title={`${c.name}: ${c.protocol.toUpperCase()} over ${c.transport}, ${c.connected ? 'connected' : 'disconnected'}, strips ${c.bankOffset + 1}-${Math.min(channels, c.bankOffset + c.strips)}`}
      data-console={c.id}
    >
      <i></i>{c.protocol.toUpperCase()}
      <span class="num fixed">{fit(c.connected ? `${c.bankOffset + 1}-${Math.min(channels, c.bankOffset + c.strips)}` : 'off', 5)}</span>
    </span>
  {/each}
</div>

<style>
  /* The chips are items of the header's right group, so a chip that
     appears moves none of the others. */
  .consoles {
    display: contents;
  }
  .c .num {
    display: inline-block;
    min-width: 5ch;
    text-align: right;
  }
  .c {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 6px;
    border-radius: 3px;
    border: 1px solid var(--line-strong);
    font-size: 10px;
    font-weight: 600;
    color: var(--text-faint);
  }
  .c i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .c.up {
    color: var(--owned);
    border-color: color-mix(in srgb, var(--owned) 60%, transparent);
  }
  .c.up i {
    background: var(--owned);
  }
  .c .num {
    font-weight: 400;
    color: var(--text-dim);
  }
  @media (max-width: 1300px) {
    .c {
      display: none;
    }
  }
</style>
