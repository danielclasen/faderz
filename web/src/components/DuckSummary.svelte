<script lang="ts">
  // The rest of the masters' duck band, beside the tiles: one line per
  // group with its name, the outputs it lowers, the targets it lowers in
  // them, its gain reduction as a bar and in dB, and its key as a bar
  // against its threshold, KEY lit while the key triggers it. Narrower
  // than SUMMARY_FULL_W the line keeps the name, the outputs, the
  // reduction in dB and the key bar. Every part has a fixed size and the
  // whole a strict containment, so no reading lays out anything outside
  // it; none of it is a live region.
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { deepestDepth, duckReach, keyShare, liveDuck, reductionShare, summaryReduction } from '../lib/duck';
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
</script>

<div class="summary" role="group" aria-label="Ducking now" data-testid="duck-summary">
  {#each groups as g, i (g.id)}
    {@const d = mixer.state!.ducks[g.id]!}
    {@const reach = duckReach(shape, d)}
    {@const gr = deepestDepth(d) * (envelopes[i] ?? 0)}
    {@const text = `${g.name}: lowers ${reach.targets} in ${reach.into}`}
    <!-- One tab stop per group: its tip carries the whole of a name, outputs or targets cut off. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="line" role="note" tabindex="0" aria-label={text} use:tip={text} data-testid={`duck-summary-${g.id}`}>
      <span class="name">{g.name}</span>
      <span class="into" data-testid={`duck-summary-into-${g.id}`}>{reach.into}</span>
      <span class="targets">{reach.targets}</span>
      <span class="bar" aria-hidden="true"><i class:on={gr > 0} style:transform={`scaleX(${reductionShare(gr, scale)})`}></i></span>
      <span class="gr" data-testid={`duck-summary-gr-${g.id}`}><span class="fixed" class:on={gr > 0}>{summaryReduction(gr)}</span></span>
      <span class="lbl db">dB</span>
      <span class="lbl keyed" class:on={engaged[i]} data-testid={`duck-summary-keyed-${g.id}`}>KEY</span>
      <span class="keybar" aria-hidden="true" data-testid={`duck-summary-key-${g.id}`}
        ><i class:on={engaged[i]} style:transform={`scaleX(${keyShare(keys[i] ?? null)})`}></i><b style:left={`${keyShare(d.threshold) * 100}%`}></b></span
      >
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
    gap: 1px;
    overflow: hidden;
    background: var(--panel-2);
    font-size: 12px;
    contain: strict;
    container-type: inline-size;
  }
  .line {
    display: grid;
    grid-template-columns: 13ch fit-content(16ch) minmax(0, 1fr) 40px 42px auto auto 44px;
    align-items: center;
    gap: 6px;
    height: 18px;
    flex: none;
  }
  .name,
  .into,
  .targets {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name,
  .into {
    font-weight: 600;
  }
  .targets {
    color: var(--text-dim);
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
    background: var(--text-dim);
  }
  .bar i.on,
  .keybar i.on {
    background: var(--duck);
  }
  /* The threshold on the key's scale. */
  .keybar b {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--text);
  }
  /* The reduction changes per meter frame: a block of fixed size with
     strict containment in its cell lays out nothing outside it, which a
     grid item of its own does not do. */
  .gr span {
    display: block;
    height: 18px;
    line-height: 18px;
    text-align: right;
    contain: strict;
  }
  /* Colour only: a weight would lay it out again at every change. */
  .gr span {
    font-weight: 600;
    color: var(--text-dim);
  }
  .gr span.on {
    color: var(--text);
  }
  .lbl {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 600;
  }
  .lbl.keyed {
    margin-left: 4px;
    padding: 0 3px;
    border-radius: 2px;
  }
  .lbl.keyed.on {
    background: var(--duck);
    color: var(--duck-ink);
  }
  /* Condensed: name, outputs, reduction in dB and the key bar, whose
     colour says the key triggers. */
  @container (width < 300px) {
    .line {
      grid-template-columns: 8ch minmax(0, 1fr) 42px 32px;
      gap: 4px;
    }
    .targets,
    .bar,
    .lbl {
      display: none;
    }
  }
</style>
