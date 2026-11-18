<script lang="ts">
  // The rest of the masters' duck band, beside the tiles: one line per
  // group with its name, what it lowers and where, its gain reduction as a
  // bar and in dB, and its key level. Every part has a fixed size, so no
  // reading moves anything; none of it is a live region.
  import { onMount } from 'svelte';
  import { tip } from '../lib/tip';
  import { GR_WIDTH, deepestDepth, duckReach, keyText, liveDuck, reductionShare, reductionText } from '../lib/duck';
  import { fit } from '../lib/format';
  import { onAnimationFrame } from '../lib/meters/store';
  import { getMixer } from '../lib/state/mixer.svelte';

  const mixer = getMixer();
  const shape = $derived(mixer.shape!);
  const groups = $derived(shape.ducks ?? []);

  // Each group's readings, polled on the frame clock; only what changed renders.
  let keys = $state<Array<number | null>>([]);
  let envelopes = $state<number[]>([]);
  onMount(() =>
    onAnimationFrame((now) => {
      groups.forEach((g, i) => {
        const m = liveDuck(mixer.meters, g.id, now);
        const key = m?.key ?? null;
        const envelope = m?.envelope ?? 0;
        if (keys[i] !== key) keys[i] = key;
        if (envelopes[i] !== envelope) envelopes[i] = envelope;
      });
    }),
  );
</script>

<div class="summary" role="group" aria-label="Duck groups now" data-testid="duck-summary">
  {#each groups as g, i (g.id)}
    {@const d = mixer.state!.ducks[g.id]!}
    {@const reach = duckReach(shape, d)}
    {@const depth = deepestDepth(d)}
    {@const gr = depth * (envelopes[i] ?? 0)}
    {@const text = `${g.name}: lowers ${reach.targets} in ${reach.into}`}
    <!-- One tab stop per group: its tip carries the whole of a name or reach cut off. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div class="line" role="note" tabindex="0" aria-label={text} use:tip={text} data-testid={`duck-summary-${g.id}`}>
      <span class="name">{g.name}</span>
      <!-- One text node: a change of what a group lowers replaces it, moving nothing. -->
      <span class="reach">{`${reach.targets} in ${reach.into}`}</span>
      <span class="bar" aria-hidden="true"><i class:on={gr > 0} style:transform={`scaleX(${reductionShare(gr, depth)})`}></i></span>
      <span class="fixed gr" class:on={gr > 0} data-testid={`duck-summary-gr-${g.id}`}>{gr > 0 ? reductionText(gr) : fit('0', GR_WIDTH)}</span>
      <span class="lbl">dB</span>
      <span class="lbl key-lbl">KEY</span>
      <span class="fixed level" data-testid={`duck-summary-key-${g.id}`}>{keyText(keys[i] ?? null)}</span>
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
    gap: 2px;
    overflow: hidden;
    background: var(--panel-2);
    font-size: 11px;
  }
  .line {
    display: grid;
    grid-template-columns: 13ch minmax(0, 1fr) 72px auto auto auto auto;
    align-items: center;
    gap: 6px;
    height: 17px;
    flex: none;
  }
  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reach {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
  }
  .bar {
    height: 8px;
    background: var(--well);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    transform-origin: left;
    background: var(--text-dim);
  }
  .bar i.on {
    background: var(--duck);
  }
  .gr.on {
    color: var(--text);
    font-weight: 600;
  }
  .key-lbl {
    margin-left: 4px;
  }
  .lbl {
    color: var(--text-dim);
    font-size: 10px;
    font-weight: 600;
  }
</style>
