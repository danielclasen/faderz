<script lang="ts">
  // Who works on this mixer: this screen, the control surfaces and every
  // other session that holds a control, with what it holds. Sessions that
  // hold nothing are not known to the client: the contract carries
  // holders (owners) and consoles, not a list of connected screens.
  import { getMixer } from '../lib/state/mixer.svelte';
  import { describePath } from '../lib/state/paths';
  import { handleTag } from '../lib/net/handle';
  import type { Owner } from '../lib/contract/types';

  const mixer = getMixer();
  let open = $state(false);
  const holders = $derived.by(() => {
    const by = new Map<string, { owner: Owner; paths: string[] }>();
    for (const [path, owner] of Object.entries(mixer.owners)) {
      if (!owner) continue;
      const k = `${owner.principal}/${owner.handle}`;
      const e = by.get(k) ?? { owner, paths: [] };
      e.paths.push(path);
      by.set(k, e);
    }
    return [...by.values()];
  });
  const consoles = $derived(mixer.state?.consoles ?? []);
  /** What a console holds goes in its own row: each console is counted and listed once. */
  const heldBy = (handle: string) => holders.find((h) => h.owner.kind === 'console' && h.owner.handle === handle);
  const others = $derived(holders.filter((h) => !mixer.isMine(h.owner) && !(h.owner.kind === 'console' && consoles.some((c) => c.handle === h.owner.handle))));
</script>

<div class="users">
  <button class="key" aria-expanded={open} aria-controls="users-panel" onclick={() => (open = !open)} title="Who works on this mixer"
    ><span class="fixed">USERS {String(others.length + consoles.filter((c) => c.connected).length).padStart(2)}</span></button
  >
  <div class="panel" class:open id="users-panel" role="region" aria-label="Users">
    <div class="row you"><b>This screen</b><span class="num">{handleTag(mixer.status.handle)}</span></div>
    {#each consoles as c (c.id)}
      {@const held = heldBy(c.handle)}
      <div class="row" data-testid="console">
        <b>{c.name}</b>
        <span
          >{c.protocol.toUpperCase()}, {c.connected ? `strips ${c.bankOffset + 1}-${c.bankOffset + c.strips}` : 'disconnected'}{held
            ? `, holds ${held.paths.map((p) => describePath(p, mixer.shape)).join(', ')}`
            : ''}</span
        >
      </div>
    {/each}
    {#each others as h (h.owner.handle)}
      <div class="row" data-testid="user">
        <b>{mixer.ownerLabel(h.owner)}</b>
        <span>holds {h.paths.map((p) => describePath(p, mixer.shape)).join(', ')}</span>
      </div>
    {/each}
    {#if others.length === 0}
      <div class="row none">No other session holds a control.</div>
    {/if}
  </div>
</div>

<style>
  .users {
    flex: none;
  }
  /* Phones: the header has room for the connection state only. */
  @media (max-width: 700px) {
    .users {
      display: none;
    }
  }
  /* Opens under the header over the view; it moves nothing. */
  .panel {
    display: none;
    position: fixed;
    top: 46px;
    right: 10px;
    z-index: 40;
    width: min(360px, calc(100vw - 20px));
    padding: 8px;
    flex-direction: column;
    gap: 6px;
    border: 1px solid var(--line-strong);
    border-radius: 3px;
    background: var(--panel);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.4);
    font-size: 11px;
    color: var(--text-dim);
  }
  .panel.open {
    display: flex;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .row b {
    color: var(--text);
    font-weight: 600;
  }
  .none {
    font-style: italic;
  }
</style>
