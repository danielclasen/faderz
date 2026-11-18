<script lang="ts" generics="Row extends { id: string; name: string }, Col extends { id: string; name: string }">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { pendingWrite } from '../lib/pending';
  // Grid of on/off cells with arrow-key navigation (one tab stop).
  interface Props {
    label: string;
    rows: Row[];
    cols: Col[];
    groups?: Array<{ name: string; span: number }>;
    get: (row: Row, col: Col) => boolean;
    toggle: (row: Row, col: Col) => void;
    /** The control a cell writes, for its pending mark. */
    path: (row: Row, col: Col) => string;
  }
  let { label, rows, cols, groups = [], get, toggle, path }: Props = $props();
  const mixer = getMixer();
  let focus = $state({ r: 0, c: 0 });
  let table: HTMLTableElement | undefined = $state();

  function key(e: KeyboardEvent, r: number, c: number): void {
    const moves: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
    const m = moves[e.key];
    if (!m) return;
    e.preventDefault();
    const nr = Math.max(0, Math.min(rows.length - 1, r + m[0]));
    const nc = Math.max(0, Math.min(cols.length - 1, c + m[1]));
    focus = { r: nr, c: nc };
    table?.querySelector<HTMLButtonElement>(`[data-cell="${nr}-${nc}"]`)?.focus();
  }
</script>

<div class="wrap">
  <table bind:this={table} aria-label={label}>
    <thead>
      {#if groups.length > 0}
        <tr>
          <th></th>
          {#each groups as g (g.name)}<th colspan={g.span} class="group">{g.name}</th>{/each}
        </tr>
      {/if}
      <tr>
        <th></th>
        {#each cols as col (col.id)}<th scope="col"><span>{col.name}</span></th>{/each}
      </tr>
    </thead>
    <tbody>
      {#each rows as row, r (row.id)}
        <tr>
          <th scope="row"><span class="num idx">{r + 1}</span>{row.name}</th>
          {#each cols as col, c (col.id)}
            {@const on = get(row, col)}
            <td>
              <button
                {@attach pendingWrite(mixer, path(row, col))}
                role="switch"
                aria-disabled={mixer.locked}
                aria-checked={on}
                aria-label={`${row.name} to ${col.name}`}
                tabindex={focus.r === r && focus.c === c ? 0 : -1}
                data-cell={`${r}-${c}`}
                onclick={() => {
                  focus = { r, c };
                  toggle(row, col);
                }}
                onkeydown={(e) => key(e, r, c)}
              ></button>
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .wrap {
    overflow: auto;
    scrollbar-gutter: stable;
    max-height: 100%;
  }
  table {
    border-collapse: separate;
    border-spacing: 0;
    font-size: 12px;
  }
  th {
    font-weight: 500;
    color: var(--text-dim);
    padding: 3px 8px;
    white-space: nowrap;
    background: var(--panel);
  }
  thead th {
    position: sticky;
    top: 0;
    z-index: 1;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    border-bottom: 1px solid var(--line-strong);
  }
  thead tr:first-child th {
    top: 0;
  }
  th.group {
    color: var(--accent);
    text-align: left;
    border-left: 1px solid var(--line-strong);
  }
  tbody th {
    position: sticky;
    left: 0;
    text-align: left;
    color: var(--text);
    border-right: 1px solid var(--line-strong);
    z-index: 0;
  }
  .idx {
    display: inline-block;
    width: 22px;
    color: var(--text-faint);
    font-size: 10px;
  }
  tbody tr:nth-child(8n) td,
  tbody tr:nth-child(8n) th {
    border-bottom: 1px solid var(--line-strong);
  }
  td {
    padding: 2px 4px;
    text-align: center;
    border-bottom: 1px solid var(--line);
  }
  td button {
    width: 30px;
    height: 20px;
    border-radius: 3px;
    border: 1px solid var(--key-edge);
    background: var(--switch-off);
    cursor: pointer;
    padding: 0;
  }
  td button[aria-checked='true'] {
    background: var(--on);
    border-color: var(--on);
  }
  tbody tr:hover th,
  tbody tr:hover td {
    background: var(--panel-2);
  }
</style>
