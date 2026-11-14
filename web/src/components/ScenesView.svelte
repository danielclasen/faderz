<script lang="ts">
  import { RECALL_GROUPS, type RecallGroup } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { recallSafePath } from '../lib/state/paths';
  import { createScene, deleteScene, recallScene, storeScene } from '../lib/state/actions';
  import Matrix from './Matrix.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const scenes = $derived(mixer.state!.scenes);
  let name = $state('');
  let crossfade = $state(500);
  let confirmDelete = $state<string | null>(null);
  let lastRecalled = $state<string | null>(null);

  const elements = $derived([...shape.channels, ...shape.buses, ...shape.auxes, ...shape.mixMinus, ...shape.vcas]);
  const groups = RECALL_GROUPS.map((g) => ({ id: g, name: g }));
  const safe = (row: { id: string }, col: { id: string }) => (mixer.state!.recallSafe[row.id] ?? []).includes(col.id as RecallGroup);
  function toggleSafe(row: { id: string }, col: { id: string }): void {
    const list = mixer.state!.recallSafe[row.id] ?? [];
    const g = col.id as RecallGroup;
    mixer.set(recallSafePath(row.id), list.includes(g) ? list.filter((x) => x !== g) : [...list, g]);
  }

  async function create(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const n = name.trim();
    if (!n) return;
    if (await createScene(mixer, n)) name = '';
  }
  async function recall(id: string): Promise<void> {
    const r = await recallScene(mixer, id, Math.max(0, Math.min(60000, Math.round(crossfade))));
    if (r) lastRecalled = id;
  }
  const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
</script>

<div class="view">
  <section class="scenes">
    <h2 class="panel-title">Scenes</h2>
    <form class="new" onsubmit={create}>
      <input type="text" bind:value={name} placeholder="New scene name" maxlength="64" aria-label="New scene name" />
      <button class="key" type="submit" disabled={!name.trim()}>STORE NEW</button>
    </form>
    <label class="xf">
      <span>Crossfade</span>
      <input type="number" min="0" max="60000" step="100" bind:value={crossfade} aria-label="Crossfade in milliseconds" />
      <span>ms</span>
    </label>
    <ol>
      {#each scenes as s, i (s.id)}
        <li class:recalled={lastRecalled === s.id} data-scene={s.id}>
          <span class="num idx">{i + 1}</span>
          <span class="name">{s.name}</span>
          <span class="num when">{time(s.storedAt)}</span>
          <button class="key go" onclick={() => recall(s.id)} aria-label={`Recall ${s.name}`}>RECALL</button>
          <button class="key" onclick={() => storeScene(mixer, s.id)} aria-label={`Store into ${s.name}`}>STORE</button>
          {#if confirmDelete === s.id}
            <button class="key mute" aria-pressed="true" onclick={() => { void deleteScene(mixer, s.id); confirmDelete = null; }}>CONFIRM</button>
          {:else}
            <button class="key" onclick={() => (confirmDelete = s.id)} aria-label={`Delete ${s.name}`}>DEL</button>
          {/if}
        </li>
      {:else}
        <li class="empty">No scenes stored.</li>
      {/each}
    </ol>
    <p class="hint">
      Recall applies to every control, held ones included, except the recall-safe groups. The mixer state takes the
      scene values at once; the audio crossfades over the crossfade time.
    </p>
  </section>
  <section class="safe">
    <h2 class="panel-title">Recall safe</h2>
    <Matrix label="Recall safe" rows={elements} cols={groups} get={safe} toggle={toggleSafe} />
  </section>
</div>

<style>
  .view {
    display: grid;
    grid-template-columns: minmax(320px, 520px) minmax(0, 1fr);
    gap: 18px;
    padding: 10px 14px;
    height: 100%;
    min-height: 0;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }
  h2 {
    margin: 0;
  }
  .new {
    display: flex;
    gap: 6px;
  }
  .new input {
    flex: 1;
  }
  .new .key {
    height: 26px;
  }
  .xf {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 11px;
    color: var(--text-dim);
  }
  .xf input {
    width: 90px;
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    border-top: 1px solid var(--line);
  }
  li {
    display: grid;
    grid-template-columns: 24px 1fr auto auto auto auto;
    gap: 6px;
    align-items: center;
    padding: 6px 4px;
    border-bottom: 1px solid var(--line);
  }
  li.recalled {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .idx,
  .when {
    color: var(--text-faint);
    font-size: 10.5px;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
  }
  .go {
    background: color-mix(in srgb, var(--accent) 20%, var(--panel-2));
    color: var(--text);
  }
  .empty {
    display: block;
    color: var(--text-faint);
  }
  .hint {
    margin: 0;
    font-size: 11px;
    color: var(--text-dim);
    max-width: 480px;
  }
  @media (max-width: 1000px) {
    .view {
      grid-template-columns: minmax(0, 1fr);
      overflow: auto;
    }
  }
</style>
