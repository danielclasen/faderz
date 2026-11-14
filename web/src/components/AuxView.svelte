<script lang="ts">
  import { getMixer } from '../lib/state/mixer.svelte';
  import { channelPath, mixMinusPath } from '../lib/state/paths';
  import { dbToPosition, formatDb, positionToDb } from '../lib/audio/db';
  import BusMaster from './BusMaster.svelte';
  import Matrix from './Matrix.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const sendPath = (ch: string, aux: string, key: string) => channelPath(ch, 'sends', aux, key);

  function level(e: Event, ch: string, aux: string): void {
    mixer.set(sendPath(ch, aux, 'level'), positionToDb(Number((e.currentTarget as HTMLInputElement).value) / 1000));
  }
  function excluded(row: { id: string }, col: { id: string }): boolean {
    return mixer.value<string[]>(mixMinusPath(col.id, 'exclude')).includes(row.id);
  }
  function toggleExclude(row: { id: string }, col: { id: string }): void {
    const list = mixer.value<string[]>(mixMinusPath(col.id, 'exclude'));
    mixer.set(mixMinusPath(col.id, 'exclude'), list.includes(row.id) ? list.filter((x) => x !== row.id) : [...list, row.id]);
  }
</script>

<div class="view">
  <div class="masters">
    {#each shape.auxes as aux (aux.id)}
      <BusMaster group="auxes" element={aux} tag="AUX" />
    {/each}
    {#each shape.mixMinus as mm (mm.id)}
      <BusMaster group="mixMinus" element={mm} tag="N-1" />
    {/each}
  </div>
  <div class="editors">
    <section>
      <h2 class="panel-title">Aux sends</h2>
      <div class="wrap">
        <table aria-label="Aux sends">
          <thead>
            <tr>
              <th></th>
              {#each shape.auxes as aux (aux.id)}<th scope="col">{aux.name}</th>{/each}
            </tr>
          </thead>
          <tbody>
            {#each shape.channels as ch, i (ch.id)}
              <tr>
                <th scope="row"><span class="num idx">{i + 1}</span>{ch.name}</th>
                {#each shape.auxes as aux (aux.id)}
                  {@const on = mixer.value<boolean>(sendPath(ch.id, aux.id, 'on'))}
                  {@const pre = mixer.value<boolean>(sendPath(ch.id, aux.id, 'pre'))}
                  {@const lvl = mixer.value<number>(sendPath(ch.id, aux.id, 'level'))}
                  <td class:off={!on}>
                    <div class="send">
                      <button class="key" aria-disabled={mixer.locked} aria-pressed={on} aria-label={`${ch.name} send to ${aux.name}`} onclick={() => mixer.set(sendPath(ch.id, aux.id, 'on'), !on)}>ON</button>
                      <input
                        type="range"
                        disabled={mixer.locked}
                        min="0"
                        max="1000"
                        value={dbToPosition(lvl) * 1000}
                        aria-label={`${ch.name} send level to ${aux.name}`}
                        aria-valuetext={`${formatDb(lvl)} dB`}
                        oninput={(e) => level(e, ch.id, aux.id)}
                        onpointerdown={() => mixer.touch(sendPath(ch.id, aux.id, 'level'))}
                        onpointerup={() => mixer.release(sendPath(ch.id, aux.id, 'level'))}
                        onpointercancel={() => mixer.release(sendPath(ch.id, aux.id, 'level'))}
                        onlostpointercapture={() => mixer.release(sendPath(ch.id, aux.id, 'level'))}
                      />
                      <span class="num db">{formatDb(lvl)}</span>
                      <button class="key" aria-disabled={mixer.locked} aria-pressed={pre} aria-label={`${ch.name} send to ${aux.name} pre-fader`} onclick={() => mixer.set(sendPath(ch.id, aux.id, 'pre'), !pre)}>{pre ? 'PRE' : 'POST'}</button>
                    </div>
                  </td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
    <section class="mm">
      <h2 class="panel-title">Mix-minus: channels left out</h2>
      <p class="hint">
        {#each shape.mixMinus as mm (mm.id)}
          <span><b>{mm.name}</b> = {shape.buses.find((b) => b.id === mm.bus)?.name ?? mm.bus} before bus processing, minus the marked channels.</span>
        {/each}
      </p>
      <Matrix label="Mix-minus exclusions" rows={shape.channels} cols={shape.mixMinus} get={excluded} toggle={toggleExclude} />
    </section>
  </div>
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .masters {
    display: flex;
    height: 52%;
    min-height: 300px;
    overflow-x: auto;
    border-bottom: 2px solid var(--line-strong);
  }
  .editors {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 16px;
    padding: 8px 14px;
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
  .hint {
    margin: 0;
    font-size: 11px;
    color: var(--text-dim);
    display: flex;
    flex-direction: column;
    max-width: 320px;
  }
  .wrap {
    overflow: auto;
    min-height: 0;
  }
  table {
    border-collapse: collapse;
    font-size: 12px;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--panel);
    color: var(--text-dim);
    font-weight: 500;
    padding: 3px 8px;
    text-align: left;
    white-space: nowrap;
    z-index: 1;
  }
  tbody th {
    left: 0;
    color: var(--text);
    z-index: 0;
  }
  .idx {
    display: inline-block;
    width: 22px;
    color: var(--text-faint);
    font-size: 10px;
  }
  td {
    padding: 2px 6px;
    border-bottom: 1px solid var(--line);
  }
  td.off .db,
  td.off input {
    opacity: 0.45;
  }
  .send {
    display: grid;
    grid-template-columns: 32px 90px 38px 40px;
    gap: 4px;
    align-items: center;
  }
  .send .key {
    padding: 0 2px;
    font-size: 9.5px;
  }
  .db {
    font-size: 10.5px;
    text-align: right;
  }
  @media (max-width: 1100px) {
    .editors {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
