<script lang="ts">
  import { onMount } from 'svelte';

  interface ReplicaRow {
    id: string;
    role: string;
    healthy: boolean;
    frozen: 'none' | 'drop' | 'apply';
    partitioned: boolean;
    clients: number;
  }
  let open = $state(false);
  let rows = $state<ReplicaRow[]>([]);

  async function refresh(): Promise<void> {
    const res = await fetch('/__mock/replicas').catch(() => null);
    if (res?.ok) rows = (await res.json()) as ReplicaRow[];
  }
  async function act(path: string): Promise<void> {
    await fetch(path, { method: 'POST' });
    await refresh();
  }
  onMount(() => {
    void refresh();
    const t = setInterval(() => void refresh(), 1000);
    return () => clearInterval(t);
  });
</script>

<aside class="mock" class:open aria-label="Mock server controls">
  <button class="toggle" onclick={() => (open = !open)} aria-expanded={open}>MOCK</button>
  {#if open}
    <div class="body">
      <p>Dev-only mock. Not part of faderz.</p>
      <table>
        <tbody>
          {#each rows as r (r.id)}
            <tr>
              <td><b>{r.id}</b></td>
              <td>{r.role}</td>
              <td>{!r.healthy ? 'down' : r.frozen !== 'none' ? `hung (${r.frozen})` : r.partitioned ? 'partitioned' : 'up'}</td>
              <td>{r.clients}c</td>
              <td>
                <button onclick={() => act(`/__mock/replicas/${r.id}/kill`)} disabled={!r.healthy}>kill</button>
                {#if r.frozen === 'none'}
                  <button onclick={() => act(`/__mock/replicas/${r.id}/freeze?mode=drop`)} disabled={!r.healthy}>hang</button>
                {:else}
                  <button onclick={() => act(`/__mock/replicas/${r.id}/unfreeze`)}>unhang</button>
                {/if}
                {#if r.partitioned}
                  <button onclick={() => act(`/__mock/replicas/${r.id}/heal`)}>heal</button>
                {:else}
                  <button onclick={() => act(`/__mock/replicas/${r.id}/partition`)} disabled={!r.healthy}>partition</button>
                {/if}
                <button onclick={() => act(`/__mock/replicas/${r.id}/restore`)} disabled={r.healthy && r.frozen === 'none' && !r.partitioned}>restore</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="row">
        <button onclick={() => act('/__mock/drop-clients')}>drop all clients</button>
        <button onclick={() => act('/__mock/skip-revision')}>skip a revision</button>
      </div>
    </div>
  {/if}
</aside>

<style>
  /* In the free stretch of the status bar between the engine lamp and the
     session; where that bar is narrow or gone, over the bank bar's
     BANKS title instead, clear of every control and readout. A phone
     has no BANKS title: there the key keeps to the status bar, just
     before the session box (8ch, after a gap of 8 px). */
  .mock {
    position: fixed;
    right: 320px;
    bottom: 3px;
    z-index: 30;
    font: 11px var(--font-num);
    color: #111;
  }
  @media (max-width: 900px), (max-height: 500px) {
    .mock {
      right: auto;
      bottom: auto;
      left: 2px;
      top: calc(var(--header-h) + 5px);
    }
    .body {
      bottom: auto;
      top: 22px;
      left: 0;
      right: auto;
      max-width: calc(100vw - 8px);
    }
  }
  @media (max-width: 700px) and (min-height: 501px) {
    .mock {
      top: auto;
      left: auto;
      right: calc(18px + 8ch * 10.5 / 11);
      bottom: 3px;
    }
    .body {
      position: fixed;
      top: auto;
      left: 4px;
      right: 4px;
      bottom: 40px;
      width: auto;
    }
  }
  @media (max-width: 700px) and (min-height: 501px) and (pointer: coarse) {
    .mock {
      bottom: 7px;
    }
  }
  .toggle {
    background: repeating-linear-gradient(45deg, #ffd400 0 6px, #111 6px 12px);
    color: #fff;
    text-shadow: 0 0 3px #000;
    border: none;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 3px;
    line-height: 14px;
    cursor: pointer;
  }
  .body {
    position: absolute;
    right: 0;
    bottom: 22px;
    width: 480px;
    background: #ffd400;
    border-radius: 4px;
    padding: 8px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
  }
  p {
    margin: 0 0 6px;
    font-weight: 600;
  }
  td {
    padding: 2px 4px;
  }
  button {
    font: inherit;
    cursor: pointer;
  }
  .row {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }
</style>
