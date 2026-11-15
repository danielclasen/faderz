<script lang="ts">
  import { RECALL_GROUPS, type RecallGroup } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { recallSafePath } from '../lib/state/paths';
  import { createScene, deleteScene, recallScene, storeScene } from '../lib/state/actions';
  import Matrix from './Matrix.svelte';
  import { tick } from 'svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const scenes = $derived(mixer.state!.scenes);
  let name = $state('');
  let crossfade = $state(500);
  let confirmDelete = $state<string | null>(null);
  /** Scene picked for the next GO; recall is a separate key, as on a console. */
  let next = $state<string | null>(null);
  let confirmStore = $state<string | null>(null);
  const current = $derived(mixer.state!.sceneRecalled?.id ?? null);
  const safeCount = $derived(Object.values(mixer.state!.recallSafe).filter((g) => g.length > 0).length);

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
    if (r) next = null;
  }
  /** An armed STORE or DEL disarms after this long, on Escape and on a press anywhere else. */
  const ARM_MS = 4000;
  let list: HTMLOListElement | undefined = $state();
  /** Focuses the key of a scene row marked data-key={key} once it is rendered. */
  async function focusKey(key: string): Promise<void> {
    await tick();
    list?.querySelector<HTMLElement>(`[data-key="${key}"]`)?.focus();
  }
  /** Arms STORE or DEL of a scene: CONFIRM takes its place and the focus, so a keyboard confirms in time. */
  function arm(kind: 'store' | 'del', id: string): void {
    confirmStore = kind === 'store' ? id : null;
    confirmDelete = kind === 'del' ? id : null;
    void focusKey(`confirm-${kind}-${id}`);
  }
  /**
   * Disarms both. A CONFIRM that had the focus hands it back to its STORE
   * or DEL key, unless a press elsewhere is moving it there.
   */
  function disarm(keepFocus = true): void {
    const focused = document.activeElement instanceof HTMLElement ? document.activeElement.dataset['key'] : undefined;
    confirmStore = null;
    confirmDelete = null;
    if (keepFocus && focused?.startsWith('confirm-')) void focusKey(focused.slice('confirm-'.length));
  }
  $effect(() => {
    if (confirmStore === null && confirmDelete === null) return;
    const t = setTimeout(disarm, ARM_MS);
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') disarm();
    };
    const press = (e: PointerEvent) => {
      if (!(e.target instanceof Element && e.target.closest('[data-confirm]'))) disarm(false);
    };
    window.addEventListener('keydown', key);
    window.addEventListener('pointerdown', press, true);
    return () => {
      clearTimeout(t);
      window.removeEventListener('keydown', key);
      window.removeEventListener('pointerdown', press, true);
    };
  });
  const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
</script>

<div class="view">
  <section class="scenes">
    <h2 class="panel-title">Scenes</h2>
    <form class="new" onsubmit={create}>
      <input type="text" bind:value={name} placeholder="New scene name" maxlength="64" aria-label="New scene name" />
      <button class="key" type="submit" disabled={!name.trim() || mixer.locked}>STORE NEW</button>
    </form>
    <label class="xf">
      <span>Crossfade</span>
      <input type="number" min="0" max="60000" step="100" bind:value={crossfade} aria-label="Crossfade in milliseconds" />
      <span>ms</span>
    </label>
    <div class="go-row">
      <span class="next-name" aria-live="polite">
        {#if next}NEXT: {scenes.find((x) => x.id === next)?.name ?? ''}{:else}Pick a scene, then GO{/if}
      </span>
      <span class="scope" title="Elements with recall-safe groups keep those groups">{safeCount} recall-safe</span>
      <button class="key go" aria-disabled={mixer.locked || next === null} disabled={next === null} onclick={() => next && recall(next)} aria-label="Recall the picked scene">GO</button>
    </div>
    <ol bind:this={list}>
      {#each scenes as s, i (s.id)}
        <li class:recalled={current === s.id} class:next={next === s.id} data-scene={s.id}>
          <span class="num idx">{i + 1}</span>
          <button class="pick" onclick={() => (next = next === s.id ? null : s.id)} aria-pressed={next === s.id} aria-label={`Pick ${s.name} for recall`}>
            <span class="name">{s.name}</span>
            <span class="tag" class:shown={current === s.id}>CURRENT</span>
          </button>
          <span class="num when">{time(s.storedAt)}</span>
          {#if confirmStore === s.id}
            <button class="key mute" aria-disabled={mixer.locked} aria-pressed="true" onclick={() => { void storeScene(mixer, s.id); disarm(); }} aria-label={`Confirm store into ${s.name}`} data-confirm data-key={`confirm-store-${s.id}`}>CONFIRM</button>
          {:else}
            <button class="key" aria-disabled={mixer.locked} onclick={() => arm('store', s.id)} aria-label={`Store into ${s.name}`} data-key={`store-${s.id}`}>STORE</button>
          {/if}
          {#if confirmDelete === s.id}
            <button class="key mute" aria-disabled={mixer.locked} aria-pressed="true" onclick={() => { void deleteScene(mixer, s.id); disarm(); }} aria-label={`Confirm delete of ${s.name}`} data-confirm data-key={`confirm-del-${s.id}`}>CONFIRM</button>
          {:else}
            <button class="key" aria-disabled={mixer.locked} onclick={() => arm('del', s.id)} aria-label={`Delete ${s.name}`} data-key={`del-${s.id}`}>DEL</button>
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
    scrollbar-gutter: stable;
    border-top: 1px solid var(--line);
  }
  li {
    display: grid;
    /* STORE and DEL columns both fit CONFIRM, which takes their place
       when armed: arming moves nothing. */
    grid-template-columns: 24px minmax(0, 1fr) 66px 9ch 66px;
    gap: 6px;
    align-items: center;
    padding: 6px 4px;
    border-bottom: 1px solid var(--line);
  }
  li.recalled {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  li.next {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .pick {
    all: unset;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    cursor: pointer;
  }
  li .key {
    width: 100%;
    padding: 0;
  }
  .pick:focus-visible {
    outline: 2px solid var(--focus);
  }
  .tag {
    visibility: hidden;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--accent-ink);
    background: var(--accent);
    border-radius: 2px;
    padding: 0 4px;
  }
  .tag.shown {
    visibility: visible;
  }
  .go-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto 8ch;
    gap: 8px;
    align-items: center;
  }
  .next-name {
    font-size: 12px;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .scope {
    font-size: 11px;
    color: var(--text-dim);
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
    height: 32px;
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
      scrollbar-gutter: stable;
    }
  }
</style>
