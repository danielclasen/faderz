<script lang="ts">
  import type { TallyState } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { afvPath } from '../lib/state/paths';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const tally = $derived(mixer.state!.tally);
  const sources = $derived(Object.keys(tally).sort());
  const label: Record<TallyState, string> = { program: 'PGM', preview: 'PVW', off: 'OFF' };
</script>

<div class="view">
  <section>
    <h2 class="panel-title">Tally from the vision switcher</h2>
    <div class="tally">
      {#each sources as s (s)}
        <div class="src {tally[s]}" data-testid={`tally-${s}`}>
          <b>{s}</b>
          <span>{label[tally[s]!]}</span>
        </div>
      {/each}
    </div>
  </section>
  <section class="bindings">
    <h2 class="panel-title">Audio follows video</h2>
    <div class="wrap">
      <table aria-label="AFV bindings">
        <thead><tr><th>#</th><th>Channel</th><th>AFV</th><th>Source</th><th>Tally</th><th>Stage</th></tr></thead>
        <tbody>
          {#each shape.channels as ch, i (ch.id)}
            {@const b = mixer.state!.afv[ch.id]!}
            {@const enabled = mixer.value<boolean>(afvPath(ch.id, 'enabled'))}
            {@const source = mixer.value<string | null>(afvPath(ch.id, 'source'))}
            {@const t = source ? tally[source] : undefined}
            <tr>
              <td class="num idx">{i + 1}</td>
              <td>{ch.name}</td>
              <td>
                <button class="key" aria-pressed={enabled} aria-label={`${ch.name} AFV`} onclick={() => mixer.set(afvPath(ch.id, 'enabled'), !enabled)}>
                  {enabled ? 'ON' : 'OFF'}
                </button>
              </td>
              <td>
                <select
                  aria-label={`${ch.name} AFV source`}
                  value={source ?? ''}
                  onchange={(e) => {
                    const v = (e.currentTarget as HTMLSelectElement).value;
                    mixer.set(afvPath(ch.id, 'source'), v === '' ? null : v);
                  }}
                >
                  <option value="">none</option>
                  {#each sources as s (s)}<option value={s}>{s}</option>{/each}
                  {#if source && !sources.includes(source)}<option value={source}>{source}</option>{/if}
                </select>
              </td>
              <td><span class="pill {t ?? 'none'}">{t ? label[t] : '-'}</span></td>
              <td><span class="stage" class:open={b.open}>{b.open ? 'open' : 'closed'}</span></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
</div>

<style>
  .view {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    gap: 12px;
    padding: 10px 14px;
    height: 100%;
    min-height: 0;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 0;
  }
  h2 {
    margin: 0;
  }
  .tally {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .src {
    width: 96px;
    border-radius: 4px;
    border: 2px solid var(--line-strong);
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    background: var(--panel-2);
  }
  .src b {
    font-size: 15px;
  }
  .src span {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--text-faint);
  }
  .src.program {
    border-color: var(--pgm);
    background: color-mix(in srgb, var(--pgm) 22%, var(--panel-2));
  }
  .src.program span {
    color: var(--pgm);
  }
  .src.preview {
    border-color: var(--pvw);
  }
  .src.preview span {
    color: var(--pvw);
  }
  .wrap {
    overflow: auto;
    min-height: 0;
  }
  table {
    border-collapse: collapse;
    font-size: 12px;
    min-width: 520px;
  }
  th,
  td {
    text-align: left;
    padding: 3px 10px;
    border-bottom: 1px solid var(--line);
  }
  th {
    position: sticky;
    top: 0;
    background: var(--bg);
    color: var(--text-faint);
    font-weight: 500;
    font-size: 11px;
    text-transform: uppercase;
  }
  .idx {
    color: var(--text-faint);
    font-size: 10px;
  }
  .pill {
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 2px;
    color: var(--text-faint);
    border: 1px solid var(--line-strong);
  }
  .pill.program {
    background: var(--pgm);
    border-color: var(--pgm);
    color: #fff;
  }
  .pill.preview {
    color: var(--pvw);
    border-color: var(--pvw);
  }
  .stage {
    font-size: 11px;
    color: var(--mute);
  }
  .stage.open {
    color: var(--ok);
  }
</style>
