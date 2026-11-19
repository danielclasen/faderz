<script lang="ts">
  // Who works on this mixer: every session connected to the primary's
  // replica (presence), this screen first, the control surfaces, and the
  // sessions on other replicas that hold a control. Each row says who is
  // signed in, the role, and what the session holds and has selected.
  import { MediaQuery } from 'svelte/reactivity';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { describePath } from '../lib/state/paths';
  import { handleTag } from '../lib/net/handle';
  import { userRows, type UserRow } from '../lib/presence';

  const mixer = getMixer();
  let open = $state(false);
  const consoles = $derived(mixer.state?.consoles ?? []);
  const rows = $derived(
    userRows(
      mixer.presence,
      mixer.owners,
      mixer.selections,
      consoles.flatMap((c) => (c.handle === null ? [] : [c.handle])),
      mixer.status.you?.handle ?? mixer.status.handle,
    ),
  );
  /** What a console holds goes in its own row: each console is counted and listed once. */
  const heldBy = (handle: string | null) =>
    Object.entries(mixer.owners)
      .filter(([, o]) => o?.kind === 'console' && o.handle === handle)
      .map(([p]) => p);
  /** A short screen (App.svelte) or a phone: USR, so the notice line and the header keys keep room. The key shows at every width: below 1301 px the panel is where the surfaces' banks are read. */
  const short = new MediaQuery('(max-height: 500px), (max-width: 700px)');
  const count = $derived(rows.length + consoles.filter((c) => c.connected).length);
  /** Surfaces configured but not connected: a warning dot on the key, which below 1301 px is the only place they are listed. */
  const down = $derived(consoles.filter((c) => !c.connected).length);
  const downText = $derived(down > 0 ? `, ${down} surface${down === 1 ? '' : 's'} disconnected` : '');
  const kindLabel = (kind: string) => (kind === 'ui' ? 'web client' : kind);
  function elementName(id: string): string {
    const s = mixer.shape;
    return [...(s?.channels ?? []), ...(s?.buses ?? []), ...(s?.auxes ?? []), ...(s?.mixMinus ?? [])].find((e) => e.id === id)?.name ?? id;
  }
  function detail(r: UserRow): string {
    const parts = [...(r.role ? [r.role] : []), `${kindLabel(r.kind)} ${handleTag(r.handle)}`];
    if (r.holds.length > 0) parts.push(`holds ${r.holds.map((p) => describePath(p, mixer.shape)).join(', ')}`);
    if (r.selected) parts.push(`selected ${elementName(r.selected)}`);
    if (r.elsewhere) parts.push('on another replica');
    return parts.join(', ');
  }
</script>

<div class="users">
  <button class="key users-key" aria-label={`Users ${count}${downText}`} aria-expanded={open} aria-controls="users-panel" onclick={() => (open = !open)} title={`Who works on this mixer${downText}`}
    ><i class="down" class:on={down > 0} aria-hidden="true" data-testid="users-down"></i><span class="count"><span class="fixed">{short.current ? 'USR' : 'USERS'} {String(count).padStart(2)}</span></span><i class="caret" aria-hidden="true"></i></button
  >
  <div class="panel" class:open id="users-panel" role="region" aria-label="Users">
    {#each rows as r (r.handle)}
      <div class="row" data-testid={r.you ? 'you' : 'user'}>
        <b>{r.user}{r.you ? ' (you)' : ''}</b>
        <span>{detail(r)}</span>
      </div>
    {/each}
    {#each consoles as c (c.id)}
      {@const held = heldBy(c.handle)}
      <div class="row" data-testid="console">
        <b>{c.name}</b>
        <span
          >{c.protocol.toUpperCase()}, {c.connected ? `strips ${c.bankOffset + 1}-${c.bankOffset + c.strips}` : 'disconnected'}{held.length > 0
            ? `, holds ${held.map((p) => describePath(p, mixer.shape)).join(', ')}`
            : ''}</span
        >
      </div>
    {/each}
    {#if rows.length === 0}
      <div class="row none">Not connected.</div>
    {/if}
  </div>
</div>

<style>
  .users {
    flex: none;
  }
  /* A control that opens a panel: a caret says so, turned while open. */
  .users-key {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
    padding-right: 7px;
  }
  .users-key[aria-expanded='true'] {
    border-color: var(--text-dim);
    color: var(--text);
  }
  /* A surface down: a dot in the key's corner, over its edge, so it
     takes no room and moves nothing. */
  .down {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    box-shadow: 0 0 0 1px var(--panel-2);
    visibility: hidden;
  }
  .down.on {
    visibility: visible;
  }
  .caret {
    flex: none;
    width: 0;
    height: 0;
    border-left: 3.5px solid transparent;
    border-right: 3.5px solid transparent;
    border-top: 4px solid currentColor;
  }
  .users-key[aria-expanded='true'] .caret {
    transform: rotate(180deg);
  }
  /* The count changes whenever a session elsewhere takes or lets go of a
     control. A button lays out its content as flex items, which are never
     relayout boundaries, so the text sits in a block of its own with size
     and layout containment: a new count lays out that box, not the page.
     The box fits a count of two digits; a third shows centred over the
     box's edges, inside the key's padding, since paint is not contained,
     and the header gives the key no more room than two digits take. */
  .count {
    display: block;
  }
  .count .fixed {
    display: flex;
    justify-content: center;
    width: calc(8ch + 8 * 0.06em);
    height: 14px;
    line-height: 14px;
    contain: size layout style;
  }
  @media (max-height: 500px), (max-width: 700px) {
    .count .fixed {
      width: calc(6ch + 6 * 0.06em);
    }
  }
  /* Opens under the header over the view; it moves nothing. */
  .panel {
    display: none;
    position: fixed;
    top: calc(var(--header-h) + 2px);
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
