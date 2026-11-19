<script lang="ts">
  import { fit } from '../lib/format';
  import { getMixer } from '../lib/state/mixer.svelte';
  const mixer = getMixer();
  const consoles = $derived(mixer.state?.consoles ?? []);
  const channels = $derived(mixer.shape?.channels.length ?? 0);
</script>

<!-- A status, not a control: one flat field per surface, its
     lamp, protocol and the strips its bank is on. Nothing here is
     pressed; the users panel lists the same surfaces at every width. -->
{#if consoles.length > 0}
  <div class="consoles" role="group" aria-label="Control surfaces" data-testid="consoles">
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
{/if}

<style>
  .consoles {
    flex: none;
    display: flex;
    align-items: stretch;
    height: var(--item-h, 26px);
    border-radius: 3px;
    background: var(--panel-2);
    box-shadow: inset 0 0 0 1px var(--line-strong);
    font-family: var(--font-num);
    font-size: 10.5px;
  }
  .c {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 8px;
    font-weight: 600;
    color: var(--text-faint);
    cursor: default;
  }
  .c + .c {
    border-left: 1px solid var(--line);
  }
  /* The lamp: lit in the surface colour while connected, an empty ring
     while not, so the state reads without colour too. */
  .c i {
    flex: none;
    box-sizing: border-box;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    border: 1px solid var(--text-faint);
  }
  .c.up {
    color: var(--owned);
  }
  .c.up i {
    border-color: var(--owned);
    background: var(--owned);
  }
  .c .num {
    display: inline-block;
    min-width: 5ch;
    text-align: right;
    font-weight: 400;
    color: var(--text-dim);
  }
  /* Below 1301 px the view tabs need the room: the users panel carries
     the surfaces and their banks. */
  @media (max-width: 1300px) {
    .consoles {
      display: none;
    }
  }
</style>
