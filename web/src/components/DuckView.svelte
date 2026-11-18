<script lang="ts">
  // Setting up duck groups: per group a column with its mode, detector,
  // presets and destinations, and per channel the group's TRIG and TARGET
  // switches and the depth. The channel grid is one tab stop; the arrows
  // move within it. A phone shows one group at a time, picked above. Up
  // to 1280 px the detector starts folded to its mode keys, so the channel
  // rows are in view. A preset rewrites a group's times and every target's
  // depth, so it takes a second press within ARM_MS or a hold.
  import { MediaQuery } from 'svelte/reactivity';
  import { pendingWrite } from '../lib/pending';
  import { fit } from '../lib/format';
  import { LAMP_WIDTH, forced, groupRefusals, lampText, targetRefusal, triggerRefusal } from '../lib/duck';
  import type { Change, DuckKeyTap, DuckShape } from '../lib/contract/types';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { patchState } from '../lib/state/actions';
  import { duckPath } from '../lib/state/paths';
  import { formatFreq } from '../lib/audio/db';
  import Param from './Param.svelte';
  import NumberEntry from './NumberEntry.svelte';
  import DuckModes from './DuckModes.svelte';
  import { hold } from '../lib/hold';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const groups = $derived(shape.ducks ?? []);
  const phone = new MediaQuery('max-width: 700px');
  const compact = new MediaQuery('max-width: 1280px');
  /** Set by the fold key; until then the detector is folded up to 1280 px. */
  let unfolded = $state<boolean | null>(null);
  const detectorOpen = $derived(unfolded ?? !compact.current);
  let picked = $state<string | null>(null);
  const shown = $derived(phone.current ? groups.filter((g) => g.id === (picked ?? groups[0]?.id)) : groups);
  const destinations = $derived([...shape.buses, ...shape.auxes, ...shape.mixMinus]);
  const TAPS: ReadonlyArray<{ id: DuckKeyTap; label: string }> = [
    { id: 'preFaderOpen', label: 'Pre-fader, while open' },
    { id: 'preFader', label: 'Pre-fader' },
    { id: 'postFader', label: 'Post-fader' },
  ];
  /** Cells per group in the channel grid: TRIG, TARGET, depth. */
  const CELLS = 3;

  /** Times of a timing preset. */
  const times = (depth: number, attackMs: number, holdMs: number, releaseMs: number) => (g: DuckShape): Change[] => {
    const d = mixer.state!.ducks[g.id]!;
    return [
      ...Object.keys(d.channels)
        .filter((c) => d.channels[c]!.target)
        .map((c) => ({ path: duckPath(g.id, 'channels', c, 'depth'), value: depth })),
      { path: duckPath(g.id, 'attackMs'), value: attackMs },
      { path: duckPath(g.id, 'holdMs'), value: holdMs },
      { path: duckPath(g.id, 'releaseMs'), value: releaseMs },
    ];
  };
  /** Presets: one atomic write each (PATCH /state). A depth goes to every current target. */
  const PRESETS = [
    { key: 'LIGHT', label: 'light: 6 dB, attack 20 ms, hold 300 ms, release 800 ms', changes: times(6, 20, 300, 800) },
    { key: 'HEAVY', label: 'heavy: 15 dB, attack 10 ms, hold 500 ms, release 1500 ms', changes: times(15, 10, 500, 1500) },
    {
      key: 'VOICE',
      label: 'voice focus: key HPF 150 Hz and LPF 4 kHz',
      changes: (g: DuckShape): Change[] => [
        { path: duckPath(g.id, 'keyHpf', 'enabled'), value: true },
        { path: duckPath(g.id, 'keyHpf', 'freq'), value: 150 },
        { path: duckPath(g.id, 'keyLpf', 'enabled'), value: true },
        { path: duckPath(g.id, 'keyLpf', 'freq'), value: 4000 },
      ],
    },
  ] as const;
  /** An armed preset disarms after this long, or when its key loses the focus. */
  const ARM_MS = 3000;
  /** A hold this long applies a preset at once. */
  const PRESET_HOLD_MS = 500;
  /** The armed preset, as `group:KEY`. */
  let armed = $state<string | null>(null);
  let armTimer: ReturnType<typeof setTimeout> | null = null;
  /**
   * A hold applied the preset: the click or key release that ends that
   * press does nothing more. Cleared at the start of the next press.
   */
  let heldOut = false;
  function disarm(): void {
    armed = null;
    if (armTimer) clearTimeout(armTimer);
    armTimer = null;
  }
  function apply(g: DuckShape, p: (typeof PRESETS)[number]): void {
    disarm();
    void patchState(mixer, `${g.name} preset ${p.key}`, p.changes(g));
  }
  /**
   * The hold action takes Space and Enter, so no click follows a key
   * press: its release presses here, and a click the browser still makes
   * from that release in the same task is not a second press.
   */
  let keyPress = false;
  function keyed(g: DuckShape, p: (typeof PRESETS)[number]): void {
    press(g, p);
    keyPress = true;
    setTimeout(() => (keyPress = false));
  }
  /** The end of a press that no hold applied: arms the preset, or applies an armed one. */
  function press(g: DuckShape, p: (typeof PRESETS)[number]): void {
    if (heldOut || mixer.locked) return;
    if (armed === `${g.id}:${p.key}`) return apply(g, p);
    disarm();
    armed = `${g.id}:${p.key}`;
    armTimer = setTimeout(disarm, ARM_MS);
  }
  $effect(() => disarm);

  /** Makes a channel a trigger or target, or not; a refused change says why instead of being sent. */
  function toggle(g: DuckShape, channel: string, key: 'trigger' | 'target'): void {
    const path = duckPath(g.id, 'channels', channel, key);
    const on = mixer.value<boolean>(path);
    const why = on ? null : (key === 'trigger' ? triggerRefusal : targetRefusal)(shape, mixer.state!, g.id, channel);
    if (why) mixer.notify('info', why, path);
    else mixer.set(path, !on);
  }
  const flag = (path: string) => mixer.value<boolean>(path);

  // One tab stop over the channel grid, as in Matrix.svelte.
  let focus = $state({ r: 0, c: 0 });
  let grid: HTMLTableSectionElement | undefined = $state();
  function move(e: KeyboardEvent, r: number, c: number): void {
    const moves: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
    const m = moves[e.key];
    if (!m) return;
    // In a depth field the side arrows move the caret, unless the whole value is selected, as on entering it.
    const field = e.currentTarget instanceof HTMLInputElement ? e.currentTarget : null;
    if (field && m[1] !== 0 && !(field.selectionStart === 0 && field.selectionEnd === field.value.length)) return;
    e.preventDefault();
    const nr = Math.max(0, Math.min(shape.channels.length - 1, r + m[0]));
    const nc = Math.max(0, Math.min(shown.length * CELLS - 1, c + m[1]));
    focus = { r: nr, c: nc };
    grid?.querySelector<HTMLElement>(`[data-cell="${nr}-${nc}"] :is(button, input)`)?.focus();
  }
  /** The focus kept within the grid shown, so one cell is always the tab stop when the groups shown change. */
  const at = $derived({ r: Math.min(focus.r, Math.max(0, shape.channels.length - 1)), c: Math.min(focus.c, Math.max(0, shown.length * CELLS - 1)) });
  const stop = (r: number, c: number) => (at.r === r && at.c === c ? 0 : -1);
  /** Why a cell's switch is refused, per shown group, counted once per group. */
  const refusals = $derived(new Map(shown.map((g) => [g.id, groupRefusals(shape, mixer.state!, g.id)])));
</script>

<div class="view">
  {#if groups.length === 0}
    <p class="empty" role="status">This shape declares no duck groups. They are declared in the shape, up to 8.</p>
  {:else}
    {#if phone.current}
      <label class="pick">
        <span class="panel-title">Group</span>
        <select aria-label="Duck group" value={shown[0]?.id} onchange={(e) => (picked = (e.currentTarget as HTMLSelectElement).value)}>
          {#each groups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
        </select>
      </label>
    {/if}
    <div class="wrap">
      <table aria-label="Duck groups" class:phone={phone.current}>
        <colgroup>
          <col class="channel" />
          {#each shown as g (g.id)}<col class="sw" /><col class="sw" /><col class="depth" />{/each}
        </colgroup>
        <thead>
          <tr class="names">
            <th></th>
            {#each shown as g (g.id)}
              {@const d = mixer.state!.ducks[g.id]!}
              <th colspan={CELLS} scope="colgroup" class="group">
                <span class="gname">{g.name}</span>
                <span class="lamp fixed" class:lit={forced(d)} class:bypass={d.mode === 'off' && !d.hold} data-testid={`duck-view-lamp-${g.id}`}>{fit(lampText(d), LAMP_WIDTH)}</span>
              </th>
            {/each}
          </tr>
          <tr class="labels">
            <th scope="col">Channel</th>
            {#each shown as g (g.id)}
              <th scope="col" class="first" title={`Keys ${g.name}`}>Trig</th>
              <th scope="col" title={`Lowered by ${g.name}`}>Target</th>
              <th scope="col" title="How far a target is lowered at full duck">Depth (dB)</th>
            {/each}
          </tr>
        </thead>
        <tbody class="setup">
          <tr>
            <th scope="row">
              <button class="key fold" aria-expanded={detectorOpen} aria-label="Detector settings" onclick={() => (unfolded = !detectorOpen)}
                ><span aria-hidden="true">{detectorOpen ? '-' : '+'}</span> Detector</button
              >
            </th>
            {#each shown as g (g.id)}
              {@const d = mixer.state!.ducks[g.id]!}
              <td colspan={CELLS} class="first">
                <div class="settings" role="group" aria-label={`${g.name} detector`}>
                  <DuckModes id={g.id} name={g.name} />
                  {#if !detectorOpen}
                    <p class="note summary">Thr {d.threshold} dB, att {d.attackMs}, hold {d.holdMs}, rel {d.releaseMs} ms</p>
                  {:else}
                  <label class="tap">
                    <span class="lbl">Key</span>
                    <select
                      disabled={mixer.locked}
                      aria-label={`${g.name} key tap`}
                      value={d.keyTap}
                      onchange={(e) => mixer.set(duckPath(g.id, 'keyTap'), (e.currentTarget as HTMLSelectElement).value)}
                    >
                      {#each TAPS as t (t.id)}<option value={t.id}>{t.label}</option>{/each}
                    </select>
                  </label>
                  <Param path={duckPath(g.id, 'threshold')} label="Thr" min={-60} max={0} step={0.5} unit=" dB" />
                  <p class="note">The key is taken after each trigger's gate and compressor.</p>
                  <Param path={duckPath(g.id, 'hysteresis')} label="Hyst" min={0} max={12} step={0.5} unit=" dB" />
                  <Param path={duckPath(g.id, 'attackMs')} label="Att" min={1} max={500} step={1} log unit=" ms" />
                  <Param path={duckPath(g.id, 'holdMs')} label="Hold" min={0} max={5000} step={10} unit=" ms" />
                  <Param path={duckPath(g.id, 'releaseMs')} label="Rel" min={10} max={10000} step={10} log unit=" ms" />
                  {#each [['keyHpf', 'HPF', 20, 500], ['keyLpf', 'LPF', 1000, 20000]] as const as [key, name, min, max] (key)}
                    <div class="filter">
                      <button
                        {@attach pendingWrite(mixer, duckPath(g.id, key, 'enabled'))}
                        class="key"
                        aria-disabled={mixer.locked}
                        aria-pressed={flag(duckPath(g.id, key, 'enabled'))}
                        aria-label={`${g.name} key ${name}`}
                        onclick={() => mixer.set(duckPath(g.id, key, 'enabled'), !flag(duckPath(g.id, key, 'enabled')))}>{name}</button
                      >
                      <Param path={duckPath(g.id, key, 'freq')} label={name} min={min} max={max} step={1} log format={(v) => `${formatFreq(v)} Hz`} />
                    </div>
                  {/each}
                  <div class="presets" role="group" aria-label={`${g.name} presets`}>
                    {#each PRESETS as p (p.key)}
                      {@const on = armed === `${g.id}:${p.key}`}
                      <button
                        class="key preset"
                        class:armed={on}
                        aria-disabled={mixer.locked}
                        aria-label={on ? `Confirm ${g.name} preset ${p.label}` : `${g.name} preset ${p.label}, press twice or hold`}
                        title={on ? 'Press again to apply' : `Preset ${p.label}: press twice or hold to apply`}
                        use:hold={{
                          ms: PRESET_HOLD_MS,
                          fire: () => {
                            if (mixer.locked) return;
                            heldOut = true;
                            apply(g, p);
                          },
                        }}
                        onpointerdown={(e) => e.button === 0 && (heldOut = false)}
                        onkeydown={(e) => (e.key === ' ' || e.key === 'Enter') && !e.repeat && (heldOut = false)}
                        onkeyup={(e) => (e.key === ' ' || e.key === 'Enter') && keyed(g, p)}
                        onclick={() => !keyPress && press(g, p)}
                        onblur={() => on && disarm()}>{on ? 'APPLY' : p.key}</button
                      >
                    {/each}
                  </div>
                  {/if}
                </div>
              </td>
            {/each}
          </tr>
          <tr>
            <th scope="row">Destinations</th>
            {#each shown as g (g.id)}
              <td colspan={CELLS} class="first">
                <div class="dests" role="group" aria-label={`${g.name} destinations`}>
                  {#each destinations as e (e.id)}
                    {@const path = duckPath(g.id, 'destinations', e.id)}
                    <button {@attach pendingWrite(mixer, path)} class="key" aria-disabled={mixer.locked} aria-pressed={flag(path)} aria-label={`${g.name} lowers its targets in ${e.name}`} title={e.name} onclick={() => mixer.set(path, !flag(path))}>{e.name}</button>
                  {/each}
                </div>
              </td>
            {/each}
          </tr>
        </tbody>
        <tbody class="members" bind:this={grid}>
          {#each shape.channels as ch, r (ch.id)}
            <tr>
              <th scope="row"><span class="num idx">{r + 1}</span>{ch.name}</th>
              {#each shown as g, gi (g.id)}
                {@const m = mixer.state!.ducks[g.id]!.channels[ch.id]!}
                {@const trig = refusals.get(g.id)?.get(ch.id)?.trigger ?? null}
                {@const tgt = refusals.get(g.id)?.get(ch.id)?.target ?? null}
                <td class="first" data-cell={`${r}-${gi * CELLS}`}>
                  <button
                    {@attach pendingWrite(mixer, duckPath(g.id, 'channels', ch.id, 'trigger'))}
                    role="switch"
                    aria-disabled={mixer.locked || trig !== null}
                    aria-checked={m.trigger}
                    aria-label={`${ch.name} triggers ${g.name}`}
                    title={trig ?? undefined}
                    tabindex={stop(r, gi * CELLS)}
                    onclick={() => {
                      focus = { r, c: gi * CELLS };
                      toggle(g, ch.id, 'trigger');
                    }}
                    onkeydown={(e) => move(e, r, gi * CELLS)}><span aria-hidden="true">{m.trigger ? 'KEY' : ''}</span></button
                  >
                </td>
                <td data-cell={`${r}-${gi * CELLS + 1}`}>
                  <button
                    {@attach pendingWrite(mixer, duckPath(g.id, 'channels', ch.id, 'target'))}
                    role="switch"
                    aria-disabled={mixer.locked || tgt !== null}
                    aria-checked={m.target}
                    aria-label={`${ch.name} is lowered by ${g.name}`}
                    title={tgt ?? undefined}
                    tabindex={stop(r, gi * CELLS + 1)}
                    onclick={() => {
                      focus = { r, c: gi * CELLS + 1 };
                      toggle(g, ch.id, 'target');
                    }}
                    onkeydown={(e) => move(e, r, gi * CELLS + 1)}><span aria-hidden="true">{m.target ? 'DK' : ''}</span></button
                  >
                </td>
                <td class="depth" class:off={!m.target} data-cell={`${r}-${gi * CELLS + 2}`}>
                  <NumberEntry
                    path={duckPath(g.id, 'channels', ch.id, 'depth')}
                    label={`${ch.name} depth under ${g.name}`}
                    min={0}
                    max={40}
                    step={0.5}
                    unit="dB"
                    compact
                    tabindex={stop(r, gi * CELLS + 2)}
                    onnav={(e) => move(e, r, gi * CELLS + 2)}
                  />
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  /* One column as wide as the view: the table scrolls in its own box
     instead of widening the view past the screen. */
  .view {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
    gap: 8px;
    padding: 10px 14px;
    height: 100%;
    box-sizing: border-box;
  }
  .empty {
    color: var(--text-dim);
    margin: 0;
  }
  .pick {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .pick select {
    flex: 1;
    min-width: 0;
  }
  .wrap {
    overflow: auto;
    scrollbar-gutter: stable;
    min-height: 0;
    grid-row: 2;
  }
  /* Fixed columns: no state change resizes one. */
  table {
    border-collapse: separate;
    border-spacing: 0;
    table-layout: fixed;
    width: max-content;
    font-size: 12px;
  }
  col.channel {
    width: 150px;
  }
  col.sw {
    width: 48px;
  }
  col.depth {
    width: 116px;
  }
  .phone col.channel {
    width: 112px;
  }
  th,
  td {
    padding: 3px 6px;
    border-bottom: 1px solid var(--line);
    background: var(--bg);
    text-align: left;
  }
  .first {
    border-left: 1px solid var(--line-strong);
  }
  /* The group names and the cell labels stay in view while the rest scrolls. */
  thead th {
    position: sticky;
    z-index: 2;
    color: var(--text-dim);
    font-weight: 500;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .names th {
    top: 0;
    height: 30px;
    box-sizing: border-box;
  }
  .labels th {
    top: 30px;
    height: 24px;
    box-sizing: border-box;
    border-bottom-color: var(--line-strong);
    text-transform: none;
  }
  th.group {
    border-left: 1px solid var(--line-strong);
    color: var(--text);
    text-transform: none;
    letter-spacing: 0;
  }
  .group {
    white-space: nowrap;
  }
  .gname {
    font-size: 13px;
    font-weight: 600;
    display: inline-block;
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    vertical-align: middle;
  }
  .lamp {
    float: right;
    font-size: 11px;
    line-height: 18px;
    padding: 0 4px;
    border-radius: 2px;
    border: 1px solid var(--line-strong);
    color: var(--text-dim);
  }
  .lamp.lit {
    background: var(--duck);
    border-color: var(--duck);
    color: var(--duck-ink);
  }
  .lamp.bypass {
    border-style: dashed;
    border-color: var(--text-dim);
    background-image: var(--hatch);
    color: var(--text);
  }
  .fold {
    width: 100%;
    text-align: left;
    padding: 0 4px;
  }
  tbody th {
    position: sticky;
    left: 0;
    z-index: 1;
    font-weight: 500;
    color: var(--text);
    border-right: 1px solid var(--line-strong);
    overflow-wrap: anywhere;
  }
  .setup th {
    vertical-align: top;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
  }
  .setup td {
    vertical-align: top;
    padding: 6px;
    background: var(--panel);
  }
  .settings {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .settings :global(.param) {
    grid-template-columns: 34px minmax(0, 1fr) auto;
  }
  .tap {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
    gap: 6px;
    align-items: center;
  }
  .tap select {
    min-width: 0;
    width: 100%;
  }
  .lbl {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .note {
    margin: 0;
    font-size: 11px;
    color: var(--text-dim);
  }
  .filter {
    display: grid;
    grid-template-columns: 40px minmax(0, 1fr);
    gap: 4px;
    align-items: center;
  }
  .filter :global(.param) {
    grid-template-columns: 0 minmax(0, 1fr) auto;
  }
  .filter :global(.param .label) {
    visibility: hidden;
  }
  .presets {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 3px;
  }
  .dests {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 3px;
  }
  .dests .key {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .idx {
    display: inline-block;
    width: 22px;
    color: var(--text-dim);
    font-size: 10px;
  }
  tbody.members tr:nth-child(8n) td,
  tbody.members tr:nth-child(8n) th {
    border-bottom: 1px solid var(--line-strong);
  }
  td button[role='switch'] {
    width: 36px;
    height: 24px;
    border-radius: 3px;
    border: 1px solid var(--key-edge);
    background: var(--switch-off);
    color: var(--on-ink);
    cursor: pointer;
    padding: 0;
    font-size: 10px;
    font-weight: 600;
  }
  td button[role='switch'][aria-checked='true'] {
    background: var(--on);
    border-color: var(--on);
  }
  /* A depth that acts on nothing yet: the channel is not a target. */
  .depth.off :global(input) {
    color: var(--text-faint);
  }
  tbody.members tr:hover th,
  tbody.members tr:hover td {
    background: var(--panel-2);
  }
  .settings .key,
  .dests .key,
  .depth :global(input) {
    height: 24px;
  }
  /* An armed preset: lit, its word APPLY in the same width. */
  .preset.armed {
    background: var(--on);
    border-color: var(--on);
    color: var(--on-ink);
  }
  .preset {
    position: relative;
    overflow: hidden;
  }
  /* The hold that applies a preset fills a bar along its bottom; transform only. */
  .preset::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--text);
    transform: scaleX(0);
    transform-origin: left;
  }
  .preset:global(.arming)::after {
    transform: scaleX(1);
    transition: transform var(--hold) linear;
  }
  /* Touch: every key, switch and depth field at least 44 px tall. */
  @media (pointer: coarse) {
    td button[role='switch'],
    .settings .key,
    .dests .key,
    .depth :global(input) {
      height: 44px;
    }
  }
</style>
