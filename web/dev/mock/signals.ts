// Simulated programme audio for the mock: a shared source model (what the
// inputs carry) and per-replica metering of it, so meter streams of
// different replicas differ by measurement jitter like real ones would.

import type { MeterValues } from '../../src/lib/contract/meterFrame.ts';
import { METER_PERIOD_MS as FRAME_MS } from '../../src/lib/contract/protocol.ts';
import type {
  BusMeter,
  ChannelMeter,
  DuckMeter,
  DuckState,
  ElementShape,
  Layout,
  Limiter,
  Loudness,
  StereoImage,
  Shape,
  State,
  PanState,
} from '../../src/lib/contract/types.ts';
import { LAYOUT_LABELS } from '../../src/lib/contract/types.ts';
import { dbToGain, effectiveGainDb } from '../../src/lib/audio/db.ts';

const SILENCE = -120;

type Kind = 'speech' | 'music' | 'vt' | 'fx' | 'film' | 'phone' | 'amb' | 'idle';

interface Source {
  kind: Kind;
  talking: boolean;
  phaseLeft: number;
  t: number;
}

function kindOf(name: string, layout: Layout): Kind {
  if (layout === '5.1') return 'film';
  if (name === 'Music') return 'music';
  if (name.startsWith('VT')) return 'vt';
  if (name === 'FX') return 'fx';
  if (name.startsWith('Phone')) return 'phone';
  if (name.startsWith('Amb')) return 'amb';
  if (name.startsWith('Spot')) return 'idle';
  return 'speech';
}

/** Source levels per input channel leg, RMS dBFS, advanced every frame. */
export class SourceModel {
  private readonly sources = new Map<string, Source>();
  readonly levels = new Map<string, number[]>();
  private readonly shape: Shape;

  constructor(shape: Shape) {
    this.shape = shape;
    for (const c of shape.channels) {
      this.sources.set(c.id, { kind: kindOf(c.name, c.layout), talking: false, phaseLeft: Math.random() * 3000, t: Math.random() * 10 });
    }
  }

  advance(): void {
    for (const c of this.shape.channels) {
      const s = this.sources.get(c.id)!;
      s.t += FRAME_MS / 1000;
      s.phaseLeft -= FRAME_MS;
      if (s.phaseLeft <= 0) {
        s.talking = !s.talking;
        s.phaseLeft = s.talking ? 1500 + Math.random() * 4500 : 600 + Math.random() * 3000;
      }
      const syllable = 4 * Math.sin(s.t * 2 * Math.PI * 4.3) + 3 * Math.sin(s.t * 2 * Math.PI * 1.7) + (Math.random() - 0.5) * 4;
      let base: number;
      switch (s.kind) {
        case 'speech': base = s.talking ? -23 + syllable : -62 + Math.random() * 3; break;
        case 'phone': base = s.talking ? -25 + syllable * 0.6 : -55 + Math.random() * 2; break;
        case 'music': base = -16 + 2 * Math.sin(s.t * 0.5) + (Math.random() - 0.5) * 2; break;
        case 'vt': base = -20 + 3 * Math.sin(s.t * 0.9) + (Math.random() - 0.5) * 3; break;
        case 'fx': base = s.talking && Math.random() < 0.3 ? -14 + Math.random() * 6 : -70; break;
        case 'film': base = -22 + 3 * Math.sin(s.t * 0.7) + (Math.random() - 0.5) * 3; break;
        case 'amb': base = -34 + 2 * Math.sin(s.t * 0.3) + (Math.random() - 0.5) * 2; break;
        default: base = -75;
      }
      const legs = c.width === 6 ? [0, 0, 2, -8, -6, -6].map((o) => base + o) : Array.from({ length: c.width }, (_, i) => base + (c.width === 2 ? (i === 0 ? 0.6 : -0.6) * Math.sin(s.t * 1.3) : 0));
      this.levels.set(c.id, legs);
    }
  }
}

/**
 * Surround panner settings of a mono source; defaults leave it at the
 * front, without spread and without LFE. The field is the ITU-R BS.775
 * layout and every law is constant power (the engine's `surround`).
 */
export interface SurroundPan {
  divergence?: number;
  /** LFE send, dB; -90 is off. */
  lfe?: number;
}

/** The power matrix of a stereo source whose legs the image has put to use first. */
function imaged(m: number[][], image: StereoImage): number[][] {
  const use = (a: number, k: number) =>
    image === 'leftToBoth' ? k === 0 : image === 'rightToBoth' ? k === 1 : image === 'swap' ? a !== k : a === k;
  return m.map((row) => [0, 1].map((k) => [0, 1].reduce((sum, a) => sum + (use(a, k) ? row[a]! : 0), 0)));
}

/** Amplitude gain of each leg of a 5.1 or 7.1 layout for a mono source at lr, fr (the engine's `surround`). */
function surroundGains(dst: Layout, lr: number, fr: number, surround: SurroundPan): Record<string, number> {
  const clamp = (v: number) => Math.min(1, Math.max(-1, v));
  const pair = (at: number): [number, number] => {
    const a = Math.min(1, Math.max(0, at)) * (Math.PI / 2);
    return [Math.cos(a), Math.sin(a)];
  };
  const p = clamp(lr);
  const [left, centre, right] = p <= 0 ? [...pair(p + 1), 0] as [number, number, number] : [0, ...pair(p)] as [number, number, number];
  // Divergence blends the pairwise front with a plain L/R pan.
  const [keep, spread] = pair(surround.divergence ?? 0);
  const [phantomL, phantomR] = pair((p + 1) / 2);
  const front = {
    L: Math.hypot(keep * left, spread * phantomL),
    R: Math.hypot(keep * right, spread * phantomR),
    C: keep * centre,
  };
  const [rl, rr] = pair((p + 1) / 2);
  const rear: Record<string, number> = dst === '7.1'
    ? { Ls: rl / Math.SQRT2, Rs: rr / Math.SQRT2, Lrs: rl / Math.SQRT2, Rrs: rr / Math.SQRT2 }
    : { Ls: rl, Rs: rr };
  const [wf, wr] = pair((clamp(fr) + 1) / 2);
  const gains: Record<string, number> = {};
  for (const [label, g] of Object.entries(front)) gains[label] = wf * g;
  for (const [label, g] of Object.entries(rear)) gains[label] = wr * g;
  const lfe = surround.lfe ?? -90;
  if (lfe > -90) gains.LFE = dbToGain(lfe);
  return gains;
}

/** The surround position of a channel's pan: front/rear, spread and LFE send (not the deprecated fr, divergence and lfe). */
function placeOf(pan: PanState): { fr: number; surround: SurroundPan } {
  return { fr: pan.frontRear ?? -1, surround: { divergence: pan.spread ?? 1, lfe: pan.lfeSend ?? -90 } };
}

/** Power gain matrix [dst][src] from one layout into another. */
export function legMatrix(src: Layout, dst: Layout, pan: number, front: number, surround: SurroundPan = {}, image: StereoImage = 'normal'): number[][] {
  const s = LAYOUT_LABELS[src];
  const d = LAYOUT_LABELS[dst];
  const m = d.map(() => s.map(() => 0));
  const set = (dl: string, sl: string, g: number) => {
    const di = d.indexOf(dl);
    const si = s.indexOf(sl);
    if (di >= 0 && si >= 0) m[di]![si]! += g * g;
  };
  const angle = ((pan + 1) * Math.PI) / 4;
  const l = Math.cos(angle);
  const r = Math.sin(angle);
  if (src === 'mono') {
    if (dst === 'mono') set('M', 'M', 1);
    else if (dst === 'stereo') { set('L', 'M', l); set('R', 'M', r); }
    else {
      for (const [label, g] of Object.entries(surroundGains(dst, pan, front, surround))) set(label, 'M', g);
    }
    return m;
  }
  if (dst === 'mono') { s.forEach((sl) => sl !== 'LFE' && set('M', sl, Math.SQRT1_2)); return m; }
  if (src === 'stereo') {
    // A stereo channel is balanced by its pan: the opposite leg lowers, the near one stays.
    set('L', 'L', Math.min(1, 1 - pan)); set('R', 'R', Math.min(1, 1 + pan));
    return imaged(m, image);
  }
  if (dst === 'stereo') {
    set('L', 'L', 1); set('R', 'R', 1); set('L', 'C', Math.SQRT1_2); set('R', 'C', Math.SQRT1_2);
    set('L', 'Ls', Math.SQRT1_2); set('R', 'Rs', Math.SQRT1_2);
    set('L', 'Lrs', 0.5); set('R', 'Rrs', 0.5);
    return m;
  }
  s.forEach((sl) => set(sl, sl, 1));
  return m;
}

const power = (db: number) => (db <= SILENCE ? 0 : Math.pow(10, db / 10));
const toDb = (p: number) => (p <= 1e-12 ? SILENCE : 10 * Math.log10(p));
const r1 = (v: number) => Math.round(Math.min(30, Math.max(SILENCE, v)) * 10) / 10;

/** 60 s of 40 ms meter periods: LRA is shown as unstable before (EBU Tech 3341). */
const LRA_STABLE_PERIODS = 1500;

/** BS.1770 momentary loudness of one set of leg levels, LFE excluded, surrounds +1.5 dB. */
function legLoudness(layout: Layout, legs: readonly number[]): number | null {
  const labels = LAYOUT_LABELS[layout];
  const z = legs.reduce((a, l, i) => {
    const label = labels[i]!;
    if (label === 'LFE') return a;
    return a + power(l) * (/^(Ls|Rs|Lrs|Rrs)$/.test(label) ? 1.41 : 1);
  }, 0);
  return z <= 1e-12 ? null : r1(-0.691 + 10 * Math.log10(z));
}

/** Phase correlation of a stereo mix, made up from how far its legs differ. */
function correlationOf(layout: Layout, legs: readonly number[]): number | null {
  if (layout !== 'stereo' || legs.every((l) => l <= -70)) return null;
  const spread = Math.min(1, Math.abs(legs[0]! - legs[1]!) / 20);
  return Math.round((0.95 - 0.8 * spread + (Math.random() - 0.5) * 0.06) * 100) / 100;
}

/** Leg levels of a stereo channel after its image setting. */
export function applyImage(image: StereoImage, legs: readonly number[]): number[] {
  const [l, r] = legs as [number, number];
  switch (image) {
    case 'leftToBoth': return [l, l];
    case 'rightToBoth': return [r, r];
    case 'swap': return [r, l];
    default: return [l, r];
  }
}

class LoudnessMeter {
  private frames: number[] = [];
  /** The latest meter periods of running time only, for the integration. */
  private runFrames: number[] = [];
  private blocks: number[] = [];
  private shortTerms: number[] = [];
  private tick = 0;
  private cachedIntegrated: number | null = null;
  private cachedLra: number | null = null;
  private maxMomentary: number | null = null;
  private maxShortTerm: number | null = null;
  /** Meter periods since the last reset. */
  private since = 0;

  reset(): void {
    this.blocks = [];
    this.shortTerms = [];
    this.maxMomentary = null;
    this.maxShortTerm = null;
    this.since = 0;
    this.cachedIntegrated = null;
    this.cachedLra = null;
  }

  /**
   * One meter period. Momentary and short-term are measured throughout
   * from `frames`. The integration (gating blocks, short-term values for
   * LRA, the maxima) reads `runFrames` instead, which holds running time
   * only: a pause drops the gating block and the 3 s window in progress,
   * and running again starts new ones, so no paused audio enters
   * integrated loudness, LRA or a maximum.
   */
  push(z: number, running: boolean): Loudness {
    this.frames.push(z);
    if (this.frames.length > 75) this.frames.shift();
    const mean = (xs: readonly number[], n: number) => {
      const w = xs.slice(-n);
      return w.reduce((a, b) => a + b, 0) / w.length;
    };
    const lufs = (zz: number) => (zz <= 1e-12 ? null : -0.691 + 10 * Math.log10(zz));
    const window = (xs: readonly number[], n: number) => (xs.length >= n ? lufs(mean(xs, n)) : null);
    const clampL = (v: number | null) => (v === null ? null : Math.round(Math.max(-120, Math.min(30, v)) * 10) / 10);
    const loudness = (): Loudness => ({
      momentary: clampL(window(this.frames, 10)),
      shortTerm: clampL(window(this.frames, 75)),
      integrated: clampL(this.cachedIntegrated),
      lra: this.cachedLra === null ? null : Math.round(Math.min(100, this.cachedLra) * 10) / 10,
      maxMomentary: this.maxMomentary,
      maxShortTerm: this.maxShortTerm,
      integratedValid: true,
      lraStable: this.since >= LRA_STABLE_PERIODS,
    });
    if (!running) {
      this.runFrames = [];
      this.tick = 0;
      return loudness();
    }
    this.runFrames.push(z);
    if (this.runFrames.length > 75) this.runFrames.shift();
    const momentary = window(this.runFrames, 10);
    const shortTerm = window(this.runFrames, 75);
    this.tick++;
    if (this.tick % 3 === 0 && this.runFrames.length >= 10) {
      this.blocks.push(mean(this.runFrames, 10));
      if (this.blocks.length > 6000) this.blocks.shift();
    }
    if (this.tick % 25 === 0) {
      if (shortTerm !== null) {
        this.shortTerms.push(shortTerm);
        if (this.shortTerms.length > 600) this.shortTerms.shift();
      }
      this.cachedIntegrated = this.integrated();
      this.cachedLra = this.lra();
    }
    const higher = (a: number | null, b: number | null) => (a === null ? b : b === null ? a : Math.max(a, b));
    this.maxMomentary = higher(this.maxMomentary, clampL(momentary));
    this.maxShortTerm = higher(this.maxShortTerm, clampL(shortTerm));
    this.since++;
    return loudness();
  }

  private integrated(): number | null {
    const abs = this.blocks.filter((z) => -0.691 + 10 * Math.log10(z) > -70);
    if (abs.length === 0) return null;
    const rel = -0.691 + 10 * Math.log10(abs.reduce((a, b) => a + b, 0) / abs.length) - 10;
    const gated = abs.filter((z) => -0.691 + 10 * Math.log10(z) > rel);
    if (gated.length === 0) return null;
    return -0.691 + 10 * Math.log10(gated.reduce((a, b) => a + b, 0) / gated.length);
  }

  private lra(): number | null {
    const abs = this.shortTerms.filter((l) => l > -70);
    if (abs.length < 3) return null;
    const meanZ = abs.reduce((a, l) => a + Math.pow(10, (l + 0.691) / 10), 0) / abs.length;
    const rel = -0.691 + 10 * Math.log10(meanZ) - 20;
    const xs = abs.filter((l) => l > rel).sort((a, b) => a - b);
    if (xs.length < 3) return null;
    const q = (p: number) => xs[Math.min(xs.length - 1, Math.floor(p * (xs.length - 1)))]!;
    return q(0.95) - q(0.1);
  }
}

interface Mix {
  rms: number[];
  peak: number[];
}

/**
 * Peak legs of a `from` element on a `to` monitor layout: equal layouts
 * copy, a mono source feeds every leg, loRo folds by the BS.775 Lo/Ro
 * equations (centre and surrounds at -3 dB, LFE dropped), lr keeps L and R.
 */
function foldPeaks(from: Layout, peaks: number[], to: Layout, downmix: 'loRo' | 'lr'): number[] {
  const src = LAYOUT_LABELS[from];
  const dst = LAYOUT_LABELS[to];
  if (from === to) return [...peaks];
  if (src.length === 1) return dst.map(() => peaks[0]!);
  const at = (label: string) => {
    const i = src.indexOf(label);
    return i < 0 ? SILENCE : peaks[i]!;
  };
  return dst.map((label) => {
    if (label === 'LFE') return SILENCE;
    const side = label.startsWith('R') ? 'R' : label.startsWith('L') ? 'L' : label;
    const parts = downmix === 'lr' ? [at(side)] : [at(side), at('C') - 3, at(`${side}s`) - 3];
    return r1(toDb(parts.reduce((a, d) => a + power(d), 0)));
  });
}

/**
 * Key level of every duck group in use when keys are 'steady', dBFS:
 * below any threshold, so a group ducks only while held or on.
 */
export const STEADY_KEY_DB = -50;

/** A duck group's detector and envelope in one replica. */
export interface Detector {
  triggered: boolean;
  /** ms the detector still holds after the key fell. */
  holdLeft: number;
  envelope: number;
}

/**
 * One meter period of a duck group's detector: triggered above the
 * threshold, released below it by the hysteresis, then engaged for holdMs
 * more; the envelope rises to 1 over attackMs while held, on or engaged
 * in auto, and falls to 0 over releaseMs otherwise.
 */
export function stepDetector(det: Detector, d: DuckState, key: number): void {
  if (key > d.threshold) det.triggered = true;
  else if (key < d.threshold - d.hysteresis) det.triggered = false;
  det.holdLeft = det.triggered ? d.holdMs : Math.max(0, det.holdLeft - FRAME_MS);
  const engaged = det.triggered || det.holdLeft > 0;
  const target = d.hold || d.mode === 'on' || (d.mode === 'auto' && engaged) ? 1 : 0;
  if (target > det.envelope) det.envelope = Math.min(1, det.envelope + FRAME_MS / d.attackMs);
  else if (target < det.envelope) det.envelope = Math.max(0, det.envelope - FRAME_MS / d.releaseMs);
}

/** Metering of one replica. */
export class ReplicaMeters {
  private readonly loudness = new Map<string, LoudnessMeter>();
  private readonly detectors = new Map<string, Detector>();
  private readonly shape: Shape;
  private readonly channelShapes: Map<string, ElementShape>;
  private readonly keys: 'triggers' | 'steady';

  /**
   * keys 'triggers': a duck group keys from the levels of its triggers,
   * so speech on them ducks it now and then; 'steady': every group in
   * use reads STEADY_KEY_DB.
   */
  constructor(shape: Shape, keys: 'triggers' | 'steady' = 'triggers') {
    this.shape = shape;
    this.channelShapes = new Map(shape.channels.map((c) => [c.id, c]));
    this.keys = keys;
  }

  resetLoudness(ids: readonly string[]): void {
    for (const id of ids) this.loudness.get(id)?.reset();
  }

  /** One meter period of this replica, in shape order. */
  frame(state: State, sources: SourceModel): MeterValues {
    const jitter = () => (Math.random() - 0.5) * 0.4;
    const channelLevels = new Map<string, number[]>();
    const channels: ChannelMeter[] = [];
    const vcaOffsets = (id: string) =>
      Object.values(state.vcas).filter((v) => v.members.includes(id)).map((v) => v.offset);
    const vcaMuted = (id: string) => Object.values(state.vcas).some((v) => v.mute && v.members.includes(id));
    for (const c of this.shape.channels) {
      const st = state.channels[c.id]!;
      const raw = sources.levels.get(c.id) ?? Array<number>(c.width).fill(SILENCE);
      const input = raw;
      const legs = input.map((l, i) => l + st.trim + (st.legTrim[i] ?? 0) + (st.comp.enabled ? st.comp.makeup - 2 : 0));
      channelLevels.set(c.id, legs);
      const fader = st.mute || vcaMuted(c.id) ? SILENCE : effectiveGainDb(st.fader, vcaOffsets(c.id));
      // The engine meters a channel after its strip, so the input tap
      // reads as the pre-fader one.
      const closed = st.meterTap === 'postFader' && fader <= SILENCE;
      const tapped = st.meterTap === 'postFader' ? legs.map((l) => (closed ? SILENCE : l + fader)) : legs;
      const direct =
        st.directOut.tap === 'input' ? input : st.directOut.tap === 'preFader' ? legs : legs.map((l) => (fader <= SILENCE ? SILENCE : l + fader));
      channels.push({
        peak: tapped.map((l) => r1(l + 9 + Math.random() * 3 + jitter())),
        truePeak: direct.map((l) => r1(l + 9.5 + Math.random() * 3 + jitter())),
        // A closed tap reads silence once the input is measured.
        momentary: closed && legLoudness(c.layout, legs) !== null ? SILENCE : legLoudness(c.layout, tapped),
      });
    }
    const postGains = new Map<string, number>();
    const post = (id: string) => {
      let gain = postGains.get(id);
      if (gain === undefined) {
        const st = state.channels[id]!;
        const afv = state.afv[id]!;
        const stage = afv.open ? 1 : dbToGain(afv.offLevel);
        gain = st.mute || vcaMuted(id) ? 0 : dbToGain(effectiveGainDb(st.fader, vcaOffsets(id))) * stage;
        postGains.set(id, gain);
      }
      return gain;
    };

    const mixInto = (layout: Layout, contributions: Array<{ id: string; gain: number; pan: number; fr: number; surround?: SurroundPan; image?: StereoImage }>): Mix => {
      const width = LAYOUT_LABELS[layout].length;
      const p = Array<number>(width).fill(0);
      const peaks = Array<number>(width).fill(SILENCE);
      for (const k of contributions) {
        if (k.gain <= 0) continue;
        const shape = this.channelShapes.get(k.id)!;
        const m = legMatrix(shape.layout, layout, k.pan, k.fr, k.surround, k.image);
        const src = channelLevels.get(k.id)!;
        for (let d = 0; d < width; d++) {
          for (let s = 0; s < src.length; s++) {
            const g2 = m[d]![s]! * k.gain * k.gain;
            if (g2 <= 0) continue;
            p[d]! += g2 * power(src[s]!);
            peaks[d] = Math.max(peaks[d]!, src[s]! + 10 * Math.log10(g2) + 10);
          }
        }
      }
      const rms = p.map(toDb);
      return { rms, peak: rms.map((r, i) => Math.max(peaks[i]!, r + 6 + Math.random() * 2)) };
    };

    const busMeter = (id: string, layout: Layout, mix: Mix, fader: number, mute: boolean, limiter?: Limiter): BusMeter => {
      const g = mute ? SILENCE : fader <= -90 ? SILENCE : fader;
      const rms = mix.rms.map((r) => (g <= SILENCE ? SILENCE : r + g));
      const peak = mix.peak.map((r) => (g <= SILENCE ? SILENCE : r + g));
      const labels = LAYOUT_LABELS[layout];
      const z = rms.reduce((a, r, i) => {
        const label = labels[i]!;
        if (label === 'LFE') return a;
        return a + power(r) * (label.startsWith('L') && label.length > 1 || label.startsWith('R') && label.length > 1 ? 1.41 : 1);
      }, 0);
      let meter = this.loudness.get(id);
      if (!meter) {
        meter = new LoudnessMeter();
        this.loudness.set(id, meter);
      }
      // An enabled limiter holds the true peak at its ceiling.
      const ceiling = limiter?.enabled ? limiter.ceiling : Infinity;
      return {
        peak: peak.map((v) => r1(Math.min(ceiling, v + jitter()))),
        truePeak: peak.map((v) => r1(Math.min(ceiling, v + Math.random() * 0.6 + jitter()))),
        rms: rms.map((v) => r1(v + jitter())),
        loudness: meter.push(z, state.loudness[id]?.running ?? true),
        correlation: correlationOf(layout, rms),
      };
    };

    const buses: BusMeter[] = [];
    for (const b of this.shape.buses) {
      const contributions = this.shape.channels
        .filter((c) => state.channels[c.id]!.routes[b.id])
        .map((c) => ({ id: c.id, gain: post(c.id), pan: state.channels[c.id]!.pan.lr, ...placeOf(state.channels[c.id]!.pan), image: state.channels[c.id]!.image }));
      const st = state.buses[b.id]!;
      buses.push(busMeter(b.id, b.layout, mixInto(b.layout, contributions), st.fader, st.mute, st.limiter));
    }
    const auxes: BusMeter[] = [];
    for (const a of this.shape.auxes) {
      const contributions = this.shape.channels.flatMap((c) => {
        const send = state.channels[c.id]!.sends[a.id];
        if (!send?.on) return [];
        const base = send.pre ? 1 : post(c.id);
        return [{ id: c.id, gain: base * dbToGain(send.level), pan: send.pan, fr: -1, surround: { divergence: 1 } }];
      });
      const st = state.auxes[a.id]!;
      auxes.push(busMeter(a.id, a.layout, mixInto(a.layout, contributions), st.fader, st.mute, st.limiter));
    }
    const mixMinus: BusMeter[] = [];
    for (const mm of this.shape.mixMinus) {
      const st = state.mixMinus[mm.id]!;
      const talk = st.interrupt.active ? st.interrupt.source : null;
      if (talk !== null && state.channels[talk]) {
        // The interrupt replaces the mix: the talkback channel before its fader.
        const pan = state.channels[talk]!.pan;
        mixMinus.push(busMeter(mm.id, mm.layout, mixInto(mm.layout, [{ id: talk, gain: 1, pan: pan.lr, ...placeOf(pan), image: state.channels[talk]!.image }]), st.fader, st.mute));
        continue;
      }
      const contributions = this.shape.channels
        .filter((c) => state.channels[c.id]!.routes[mm.bus] && !st.exclude.includes(c.id))
        .map((c) => ({ id: c.id, gain: post(c.id), pan: state.channels[c.id]!.pan.lr, ...placeOf(state.channels[c.id]!.pan), image: state.channels[c.id]!.image }));
      mixMinus.push(busMeter(mm.id, mm.layout, mixInto(mm.layout, contributions), st.fader, st.mute));
    }
    // Line-up tone replaces the mix of its targets: a sine at level dBFS
    // peak reads level - 3.01 dB RMS.
    if (state.tone.on) {
      const tone = (list: BusMeter[], shapes: ElementShape[]) =>
        shapes.forEach((e, i) => {
          if (!state.tone.targets.includes(e.id)) return;
          const m = list[i]!;
          const legs = m.peak.map(() => state.tone.level);
          list[i] = { ...m, peak: legs, truePeak: legs, rms: legs.map((l) => r1(l - 3.01)) };
        });
      tone(buses, this.shape.buses);
      tone(auxes, this.shape.auxes);
      tone(mixMinus, this.shape.mixMinus);
    }

    // Monitor feed: the listen sum, PFL over AFL, else the source folded. PFL is
    // before the pan, so it plays centred; AFL plays at the channel's pan.
    const monitor = this.shape.monitorOutputs.map((out) => {
      const solo = (id: string) => {
        const own = state.channels[id]!.solo;
        if (own !== 'off') return own;
        const vca = Object.values(state.vcas).find((v) => v.solo !== 'off' && v.members.includes(id));
        return vca?.solo ?? 'off';
      };
      const listens = this.shape.channels.map((c) => ({ id: c.id, mode: solo(c.id) })).filter((l) => l.mode !== 'off');
      const pfl = listens.some((l) => l.mode === 'pfl');
      if (listens.length > 0) {
        const heard = listens.filter((l) => l.mode === (pfl ? 'pfl' : 'afl'));
        const gain = (id: string) => {
          const st = state.channels[id]!;
          if (pfl) return 1;
          return st.mute || vcaMuted(id) ? 0 : dbToGain(effectiveGainDb(st.fader, vcaOffsets(id)));
        };
        const mix = mixInto(out.layout, heard.map((l) => ({ id: l.id, gain: gain(l.id), ...(pfl ? { pan: 0, fr: -1, surround: { divergence: 1 }, image: state.channels[l.id]!.image } : { pan: state.channels[l.id]!.pan.lr, ...placeOf(state.channels[l.id]!.pan), image: state.channels[l.id]!.image }) })));
        const peak = mix.peak.map((v) => r1(Math.max(SILENCE, v + jitter())));
        return { peak: out.layout === 'stereo' ? applyImage(state.monitor.image, peak) : peak };
      }
      const all: Array<[ElementShape, BusMeter]> = [
        ...this.shape.buses.map((e, i) => [e, buses[i]!] as [ElementShape, BusMeter]),
        ...this.shape.auxes.map((e, i) => [e, auxes[i]!] as [ElementShape, BusMeter]),
        ...this.shape.mixMinus.map((e, i) => [e, mixMinus[i]!] as [ElementShape, BusMeter]),
      ];
      const found = all.find(([e]) => e.id === state.monitor.source);
      const peak = found ? foldPeaks(found[0].layout, found[1].peak, out.layout, state.monitor.downmix) : LAYOUT_LABELS[out.layout].map(() => SILENCE);
      return { peak: out.layout === 'stereo' ? applyImage(state.monitor.image, peak) : peak };
    });

    // A group is in use here while it has a trigger and a target. Its key
    // is the loudest trigger leg as a peak, at the gain its keyTap gives.
    const ducks = (this.shape.ducks ?? []).map((g): DuckMeter => {
      const d = state.ducks[g.id]!;
      const members = Object.entries(d.channels);
      if (!members.some(([, m]) => m.trigger) || !members.some(([, m]) => m.target)) {
        this.detectors.delete(g.id);
        return { key: null, envelope: 0, engaged: false };
      }
      let key = STEADY_KEY_DB;
      if (this.keys === 'triggers') {
        key = SILENCE;
        for (const [id, m] of members) {
          if (!m.trigger) continue;
          const gain = d.keyTap === 'preFader' ? 1 : d.keyTap === 'postFader' ? post(id) : post(id) > 0 ? 1 : 0;
          if (gain <= 0) continue;
          for (const l of channelLevels.get(id) ?? []) key = Math.max(key, l + 9 + 20 * Math.log10(gain));
        }
      }
      let det = this.detectors.get(g.id);
      if (!det) this.detectors.set(g.id, (det = { triggered: false, holdLeft: 0, envelope: 0 }));
      stepDetector(det, d, key);
      return { key: r1(key), envelope: Math.round(det.envelope * 10000) / 10000, engaged: det.triggered || det.holdLeft > 0 };
    });
    return { channels, buses, auxes, mixMinus, monitor, ducks };
  }
}
