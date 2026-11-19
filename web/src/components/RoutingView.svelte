<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { channelPath, vcaPath } from '../lib/state/paths';
  import { TAP_LABEL } from '../lib/audio/taps';
  import type { Layout } from '../lib/contract/types';
  import Matrix from './Matrix.svelte';
  import LayoutBadge from './LayoutBadge.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  type Col = { id: string; name: string; kind: 'bus' | 'vca'; layout?: Layout };
  const cols = $derived<Col[]>([
    ...shape.buses.map((b) => ({ id: b.id, name: b.name, kind: 'bus' as const, layout: b.layout })),
    ...shape.vcas.map((v) => ({ id: v.id, name: v.name, kind: 'vca' as const })),
  ]);

  function get(row: { id: string }, col: Col): boolean {
    return col.kind === 'bus'
      ? mixer.value<boolean>(channelPath(row.id, 'routes', col.id))
      : mixer.value<string[]>(vcaPath(col.id, 'members')).includes(row.id);
  }
  function toggle(row: { id: string }, col: Col): void {
    if (col.kind === 'bus') {
      mixer.set(channelPath(row.id, 'routes', col.id), !get(row, col));
    } else {
      const members = mixer.value<string[]>(vcaPath(col.id, 'members'));
      mixer.set(vcaPath(col.id, 'members'), members.includes(row.id) ? members.filter((m) => m !== row.id) : [...members, row.id]);
    }
  }
</script>

<div class="view">
  <section class="grid">
    <h2 class="panel-title">Channel routing and VCA groups</h2>
    <Matrix
      label="Routing"
      rows={shape.channels}
      {cols}
      groups={[{ name: 'Buses', span: shape.buses.length }, { name: 'VCA', span: shape.vcas.length }]}
      {get}
      {toggle}
      path={(row, col) => (col.kind === 'bus' ? channelPath(row.id, 'routes', col.id) : vcaPath(col.id, 'members'))}
    />
  </section>
  <section class="outputs">
    <h2 class="panel-title">Output flows (configured)</h2>
    <table>
      <thead><tr><th>Output</th><th>Carries</th><th>Layout</th></tr></thead>
      <tbody>
        {#each shape.outputs as o (o.id)}
          {@const src = [...shape.buses, ...shape.auxes, ...shape.mixMinus, ...shape.monitorOutputs].find((e) => e.id === o.source)}
          {@const direct = shape.channels.find((c) => c.id === o.source)}
          <tr>
            <td>{o.name}</td>
            <td>{#if direct}{direct.name} direct out, {TAP_LABEL[mixer.state!.channels[direct.id]!.directOut.tap]}{:else}{src?.name ?? o.source}{/if}</td>
            <td class="num">{#if o.layout === 'mono'}mono{:else}<LayoutBadge layout={o.layout} />{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </section>
</div>

<style>
  .view {
    display: grid;
    grid-template-columns: minmax(0, auto) minmax(240px, 320px);
    gap: 16px;
    padding: 10px 14px;
    height: 100%;
    min-height: 0;
  }
  section {
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  h2 {
    margin: 0;
  }
  .outputs table {
    border-collapse: collapse;
    font-size: 12px;
  }
  .outputs th,
  .outputs td {
    text-align: left;
    padding: 4px 8px;
    border-bottom: 1px solid var(--line);
  }
  .outputs th {
    color: var(--text-faint);
    font-weight: 500;
    font-size: 11px;
    text-transform: uppercase;
  }
  /* Narrow: both sections stack and the view scrolls, so no output row
     is cut off below the screen. */
  @media (max-width: 900px) {
    .view {
      grid-template-columns: minmax(0, 1fr);
      overflow: auto;
      scrollbar-gutter: stable;
    }
  }
</style>
