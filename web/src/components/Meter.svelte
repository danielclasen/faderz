<script lang="ts">
  import { onMount } from 'svelte';
  import { getMixer } from '../lib/state/mixer.svelte';
  import { onAnimationFrame, type MeterGroup } from '../lib/meters/store';
  import { METER_FLOOR_DB, METER_MARKS, meterPosition, newLeg, stepLeg, type LegBallistics } from '../lib/meters/ballistics';

  interface Props {
    group: MeterGroup;
    id: string;
    legs: number;
    scale?: boolean;
    label?: string;
  }
  let { group, id, legs, scale = false, label = 'meter' }: Props = $props();

  const mixer = getMixer();
  let canvas: HTMLCanvasElement | undefined = $state();

  const STALE_MS = 500;
  const MID = -18;
  const HIGH = -6;

  onMount(() => {
    const el = canvas!;
    const ctx = el.getContext('2d')!;
    let peak: LegBallistics[] = [];
    let rms: LegBallistics[] = [];
    let w = 0;
    let h = 0;
    let dpr = 1;
    const resize = () => {
      dpr = window.devicePixelRatio || 1;
      w = el.clientWidth;
      h = el.clientHeight;
      el.width = Math.max(1, Math.round(w * dpr));
      el.height = Math.max(1, Math.round(h * dpr));
    };
    const ro = new ResizeObserver(resize);
    ro.observe(el);
    resize();
    const style = getComputedStyle(document.documentElement);
    let colors = { low: '', mid: '', high: '', well: '', line: '', text: '' };
    const readColors = () => {
      colors = {
        low: style.getPropertyValue('--meter-low').trim(),
        mid: style.getPropertyValue('--meter-mid').trim(),
        high: style.getPropertyValue('--meter-high').trim(),
        well: style.getPropertyValue('--well').trim(),
        line: style.getPropertyValue('--line-strong').trim(),
        text: style.getPropertyValue('--text-faint').trim(),
      };
    };
    readColors();
    const mo = new MutationObserver(readColors);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

    const scaleW = scale ? 18 : 0;
    const bar = (x: number, bw: number, fromDb: number, toDb: number, color: string, alpha: number) => {
      const y0 = h - meterPosition(fromDb) * h;
      const y1 = h - meterPosition(toDb) * h;
      if (y0 - y1 <= 0) return;
      ctx.globalAlpha = alpha;
      ctx.fillStyle = color;
      ctx.fillRect(x, y1, bw, y0 - y1);
    };
    const zoned = (x: number, bw: number, value: number, alpha: number) => {
      bar(x, bw, METER_FLOOR_DB, Math.min(value, MID), colors.low, alpha);
      if (value > MID) bar(x, bw, MID, Math.min(value, HIGH), colors.mid, alpha);
      if (value > HIGH) bar(x, bw, HIGH, value, colors.high, alpha);
    };
    const zoneColor = (v: number) => (v > HIGH ? colors.high : v > MID ? colors.mid : colors.low);

    const stop = onAnimationFrame((now, dt) => {
      if (w === 0 || h === 0) return;
      const reading = mixer.meters.get(group, id);
      const fresh = reading && now - mixer.meters.receivedAt < STALE_MS;
      if (peak.length !== legs) {
        peak = Array.from({ length: legs }, newLeg);
        rms = Array.from({ length: legs }, newLeg);
      }
      for (let i = 0; i < legs; i++) {
        stepLeg(peak[i]!, fresh ? (reading.peak[i] ?? METER_FLOOR_DB) : METER_FLOOR_DB, now, dt);
        stepLeg(rms[i]!, fresh ? (reading.rms[i] ?? METER_FLOOR_DB) : METER_FLOOR_DB, now, dt);
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.globalAlpha = 1;
      ctx.fillStyle = colors.well;
      ctx.fillRect(0, 0, w, h);
      const inner = w - scaleW;
      const gap = legs > 1 ? 1 : 0;
      const bw = Math.max(1, (inner - gap * (legs - 1)) / legs);
      for (let i = 0; i < legs; i++) {
        const x = scaleW + i * (bw + gap);
        zoned(x, bw, peak[i]!.bar, 0.45);
        zoned(x, bw, rms[i]!.bar, 1);
        const hold = peak[i]!.hold;
        if (hold > METER_FLOOR_DB) {
          ctx.globalAlpha = 1;
          ctx.fillStyle = zoneColor(hold);
          ctx.fillRect(x, h - meterPosition(hold) * h - 1, bw, 2);
        }
      }
      ctx.globalAlpha = 1;
      ctx.fillStyle = colors.line;
      for (const mark of METER_MARKS) {
        const y = Math.round(h - meterPosition(mark) * h) + 0.5;
        ctx.fillRect(scaleW, y, inner, mark === MID ? 1 : 0.5);
      }
      if (scale) {
        ctx.fillStyle = colors.text;
        ctx.font = '8px "JetBrains Mono", monospace';
        ctx.textAlign = 'right';
        ctx.textBaseline = 'middle';
        for (const mark of METER_MARKS) {
          const y = Math.min(h - 5, Math.max(5, h - meterPosition(mark) * h));
          ctx.fillText(String(mark), scaleW - 3, y);
        }
      }
    });
    return () => {
      stop();
      ro.disconnect();
      mo.disconnect();
    };
  });
</script>

<div class="meter" role="img" aria-label={label} data-meter={`${group}/${id}`}><canvas bind:this={canvas} aria-hidden="true"></canvas></div>

<style>
  /* The canvas is taken out of flow so its pixel size never feeds back
     into the layout that sizes it. */
  .meter {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
  }
  canvas {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
    border-radius: 2px;
  }
</style>
