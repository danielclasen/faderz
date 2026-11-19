<script lang="ts">
  // The rest of the masters' duck band, beside the tiles: per group two
  // rows, the group's name heading both. The first says what keys it,
  // "key: <triggers> (<tap>)", its key level against its threshold, KEY
  // lit while the key triggers it, and the key as a bar against the
  // threshold; the second what it lowers, "ducks: <targets> on
  // <outputs>", its gain reduction in dB and as a bar. Narrower than
  // 420 px the bars and the key level go and the name heads the first
  // row only. Every
  // part has a fixed size and the whole a strict containment, so no
  // reading lays out anything outside it; none of it is a live region.
  // Focused or pointed at, a line's tip adds its readings of that moment.
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { deepestDepth, duckReach, keyShare, keyingText, keyingWords, liveDuck, reductionShare, summaryReduction } from '../lib/duck';
  import { onAnimationFrame } from '../lib/meters/store';
  import { getMixer } from '../lib/state/mixer.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const groups = $derived(shape.ducks ?? []);
  /** One reduction scale for every line: the deepest depth of any group. */
  const scale = $derived(Math.max(0, ...groups.map((g) => deepestDepth(mixer.state!.ducks[g.id]!))));

  // Each group's readings, polled on the frame clock; only what changed renders.
  let keys = $state<Array<number | null>>([]);
  let envelopes = $state<number[]>([]);
  let engaged = $state<boolean[]>([]);
  onMount(() =>
    onAnimationFrame((now) => {
      groups.forEach((g, i) => {
        const m = liveDuck(mixer.meters, g.id, now);
        const key = m?.key ?? null;
        const envelope = m?.envelope ?? 0;
        const on = m?.engaged ?? false;
        if (keys[i] !== key) keys[i] = key;
        if (envelopes[i] !== envelope) envelopes[i] = envelope;
        if (engaged[i] !== on) engaged[i] = on;
      });
    }),
  );
  /** Each line's readings when it took focus or the pointer: they change per meter frame, its tip only then. */
  let readings = $state<string[]>([]);
  function describe(i: number, depth: number, threshold: number): void {
    const gr = depth * (envelopes[i] ?? 0);
    readings[i] = `Reduction ${summaryReduction(gr).trim()} dB, ${keyingWords(keys[i] ?? null, threshold)}${engaged[i] ? ', keyed' : ''}`;
  }
</script>

<div class="summary" role="group" aria-label="Ducking now" data-testid="duck-summary">
  {#each groups as g, i (g.id)}
    {@const d = mixer.state!.ducks[g.id]!}
    {@const reach = duckReach(shape, d)}
    {@const depth = deepestDepth(d)}
    {@const gr = depth * (envelopes[i] ?? 0)}
    {@const text = `${g.name}. key: ${reach.triggers} (${reach.tap}). ducks: ${reach.targets} on ${reach.into}`}
    <!-- One tab stop per group, named by the group: its tip, the description
         while it shows, carries the whole of a name, triggers, targets or
         outputs cut off and the readings when it took focus or the pointer. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="line"
      role="note"
      tabindex="0"
      aria-label={g.name}
      onfocus={() => describe(i, depth, d.threshold)}
      onpointerenter={() => describe(i, depth, d.threshold)}
      use:tip={readings[i] ? `${text}. ${readings[i]}` : text}
      data-testid={`duck-summary-${g.id}`}
    >
      <span class="name">{g.name}</span>
      <span class="fact keys" data-testid={`duck-summary-keys-${g.id}`}><span class="k">key:</span> {reach.triggers} <span class="tap">({reach.tap})</span></span>
      <span class="keying" data-testid={`duck-summary-keying-${g.id}`}><span class="fixed" class:on={engaged[i]}>{keyingText(keys[i] ?? null, d.threshold)}</span></span>
      <span class="lbl keyed" class:on={engaged[i]} data-testid={`duck-summary-keyed-${g.id}`}>KEY</span>
      <span class="keybar" aria-hidden="true" data-testid={`duck-summary-key-${g.id}`}
        ><i class:on={engaged[i]} style:transform={`scaleX(${keyShare(keys[i] ?? null)})`}></i><i
          class="threshold"
          style:transform={`translateX(${keyShare(d.threshold) * 100}%)`}><b></b></i></span
      >
      <span class="fact ducks" data-testid={`duck-summary-ducks-${g.id}`}
        ><span class="targets"><span class="k">ducks:</span> {reach.targets}</span><span class="into" data-testid={`duck-summary-into-${g.id}`}>{' '}on {reach.into}</span></span
      >
      <span class="gr" data-testid={`duck-summary-gr-${g.id}`}><span class="fixed" class:on={gr > 0}>{summaryReduction(gr)}</span></span>
      <span class="lbl db">dB</span>
      <span class="bar" aria-hidden="true"><i class:on={gr > 0} style:transform={`scaleX(${reductionShare(gr, scale)})`}></i></span>
    </div>
  {/each}
</div>

<style>
  .summary {
    flex: 1 1 0;
    min-width: 0;
    height: var(--tile-h);
    padding: 5px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow: hidden;
    background: var(--panel-2);
    font-size: 12px;
    contain: strict;
    container-type: inline-size;
  }
  /* Two rows a group under its name: what keys it and why, then what it
     lowers and by how much. The readings and the bars stand in columns
     of their own, so every group's line up. */
  .line {
    flex: none;
    display: grid;
    grid-template-columns: 13ch minmax(0, 1fr) auto 26px 44px;
    grid-template-rows: 16px 15px;
    grid-template-areas:
      'name keys keying keyed kbar'
      '. ducks gr db bar';
    align-items: center;
    column-gap: 6px;
  }
  .name {
    grid-area: name;
    font-weight: 600;
  }
  .keys {
    grid-area: keys;
  }
  /* Sized in its own monospace digits: KEYING_WIDTH characters. */
  .keying {
    grid-area: keying;
    height: 100%;
    width: 14ch;
    font-family: var(--font-num);
    font-size: 11px;
  }
  .keyed {
    grid-area: keyed;
    justify-self: start;
  }
  .keybar {
    grid-area: kbar;
  }
  .ducks {
    grid-area: ducks;
    display: flex;
    min-width: 0;
  }
  .gr {
    grid-area: gr;
    height: 100%;
  }
  .db {
    grid-area: db;
  }
  .bar {
    grid-area: bar;
  }
  .name,
  .keys,
  .targets {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .fact {
    font-size: 11px;
    color: var(--text);
  }
  /* The labels say what each row is; the outputs never shrink, the
     targets before them give way first. */
  .k {
    color: var(--text-dim);
    font-weight: 600;
  }
  .tap {
    color: var(--text-dim);
  }
  .targets {
    flex: 0 1 auto;
    min-width: 2ch;
  }
  .into {
    flex: none;
    margin-left: 0.3em;
    white-space: nowrap;
    font-weight: 600;
  }
  .bar,
  .keybar {
    position: relative;
    height: 8px;
    background: var(--well);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar i,
  .keybar i {
    display: block;
    height: 100%;
    transform-origin: left;
    background: var(--well-dim);
  }
  .bar i.on,
  .keybar i.on {
    background: var(--well-duck);
  }
  /* The threshold on the key's scale, edged in the well's colour so it
     shows over a lit fill too. A bar-wide box moved by transform, as the
     loudness target is: a new threshold changes no layout. */
  .keybar i.threshold {
    position: absolute;
    inset: 0;
    background: none;
  }
  .keybar b {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--well-mark);
    box-shadow:
      -1px 0 0 var(--well),
      1px 0 0 var(--well);
  }
  /* The readings change per meter frame: a block of fixed size with
     strict containment in its cell lays out nothing outside it, which a
     grid item of its own does not do. Colour only: a weight change would
     lay it out again. */
  .gr span,
  .keying span {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    height: 100%;
    font-weight: 600;
    color: var(--text-dim);
    contain: strict;
    white-space: pre;
  }
  .keying span {
    font-weight: 400;
  }
  .gr span.on,
  .keying span.on {
    color: var(--text);
  }
  .lbl {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 600;
  }
  .lbl.keyed {
    padding: 0 3px;
    border-radius: 2px;
  }
  .lbl.keyed.on {
    background: var(--duck);
    color: var(--duck-ink);
  }
  /* Narrower: the name heads the key row only, the ducks row takes the
     whole width but its reduction and KEY, and the bars and the key
     level go; the tip carries the level. 420 px of summary, less its
     padding: the container's content box. */
  @container (width < 404px) {
    .line {
      grid-template-columns: 8ch minmax(0, 1fr) 36px auto auto;
      grid-template-areas:
        'name keys keys keys keyed'
        'ducks ducks gr db .';
      column-gap: 4px;
    }
    .gr span {
      font-size: 11px;
    }
    .keying,
    .bar,
    .keybar {
      display: none;
    }
  }
</style>
