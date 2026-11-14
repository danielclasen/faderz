// Simulated programme audio for the mock: a shared source model (what the
// inputs carry) and per-replica metering of it, so meter streams of
// different replicas differ by measurement jitter like real ones would.

import type { MeterValues } from '../../src/lib/contract/meterFrame.ts';
import { METER_PERIOD_MS as FRAME_MS } from '../../src/lib/contract/protocol.ts';
import type {
  BusMeter,
  ChannelMeter,
  ElementShape,
  Layout,
  Limiter,
  Loudness,
  StereoImage,
  Shape,
  State,
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

/** Surround panner settings of a mono source; defaults leave it front, discrete, without LFE. */
export interface SurroundPan {
  divergence?: number;
  /** LFE send, dB; -90 is off. */
  lfe?: number;
}

/** Power gain matrix [dst][src] from one layout into another. */
export function legMatrix(src: Layout, dst: Layout, pan: number, front: number, surround: SurroundPan = {}): number[][] {
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
      const rear = (front + 1) / 2;
      const fr = Math.cos((rear * Math.PI) / 2);
      const rr = Math.sin((rear * Math.PI) / 2);
      // A centred source goes to the centre leg as far as divergence allows.
      const centre = (1 - (surround.divergence ?? 0)) * (1 - Math.abs(pan));
      const side = Math.sqrt(1 - centre);
      set('C', 'M', Math.sqrt(centre) * fr);
      set('L', 'M', l * fr * side); set('R', 'M', r * fr * side); set('Ls', 'M', l * rr); set('Rs', 'M', r * rr);
      if ((surround.lfe ?? -90) > -90) set('LFE', 'M', dbToGain(surround.lfe!));
    }
    return m;
  }
  if (dst === 'mono') { s.forEach((sl) => sl !== 'LFE' && set('M', sl, Math.SQRT1_2)); return m; }
  if (src === 'stereo') { set('L', 'L', 1); set('R', 'R', 1); return m; }
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

  push(z: number): Loudness {
    this.frames.push(z);
    if (this.frames.length > 75) this.frames.shift();
    const mean = (n: number) => {
      const xs = this.frames.slice(-n);
      return xs.reduce((a, b) => a + b, 0) / xs.length;
    };
    const lufs = (zz: number) => (zz <= 1e-12 ? null : -0.691 + 10 * Math.log10(zz));
    const momentary = this.frames.length >= 10 ? lufs(mean(10)) : null;
    const shortTerm = this.frames.length >= 75 ? lufs(mean(75)) : null;
    this.tick++;
    if (this.tick % 3 === 0 && this.frames.length >= 10) {
      this.blocks.push(mean(10));
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
    const clampL = (v: number | null) => (v === null ? null : Math.round(Math.max(-120, Math.min(30, v)) * 10) / 10);
    const higher = (a: number | null, b: number | null) => (a === null ? b : b === null ? a : Math.max(a, b));
    this.maxMomentary = higher(this.maxMomentary, clampL(momentary));
    this.maxShortTerm = higher(this.maxShortTerm, clampL(shortTerm));
    this.since++;
    return {
      momentary: clampL(momentary),
      shortTerm: clampL(shortTerm),
      integrated: clampL(this.cachedIntegrated),
      lra: this.cachedLra === null ? null : Math.round(Math.min(100, this.cachedLra) * 10) / 10,
      maxMomentary: this.maxMomentary,
      maxShortTerm: this.maxShortTerm,
      integratedValid: true,
      lraStable: this.since >= LRA_STABLE_PERIODS,
    };
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

/** Metering of one replica. */
export class ReplicaMeters {
  private readonly loudness = new Map<string, LoudnessMeter>();
  private readonly shape: Shape;
  private readonly channelShapes: Map<string, ElementShape>;

  constructor(shape: Shape) {
    this.shape = shape;
    this.channelShapes = new Map(shape.channels.map((c) => [c.id, c]));
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
      const input = c.layout === 'stereo' ? applyImage(st.image, raw) : raw;
      const legs = input.map((l, i) => l + st.trim + (st.legTrim[i] ?? 0) + (st.comp.enabled ? st.comp.makeup - 2 : 0));
      channelLevels.set(c.id, legs);
      const fader = st.mute || vcaMuted(c.id) ? SILENCE : effectiveGainDb(st.fader, vcaOffsets(c.id));
      const tapped =
        st.meterTap === 'input' ? input : st.meterTap === 'preFader' ? legs : legs.map((l) => (fader <= SILENCE ? SILENCE : l + fader));
      const direct =
        st.directOut.tap === 'input' ? input : st.directOut.tap === 'preFader' ? legs : legs.map((l) => (fader <= SILENCE ? SILENCE : l + fader));
      channels.push({
        peak: tapped.map((l) => r1(l + 9 + Math.random() * 3 + jitter())),
        truePeak: direct.map((l) => r1(l + 9.5 + Math.random() * 3 + jitter())),
        momentary: legLoudness(c.layout, tapped),
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

    const mixInto = (layout: Layout, contributions: Array<{ id: string; gain: number; pan: number; fr: number; surround?: SurroundPan }>): Mix => {
      const width = LAYOUT_LABELS[layout].length;
      const p = Array<number>(width).fill(0);
      const peaks = Array<number>(width).fill(SILENCE);
      for (const k of contributions) {
        if (k.gain <= 0) continue;
        const shape = this.channelShapes.get(k.id)!;
        const m = legMatrix(shape.layout, layout, k.pan, k.fr, k.surround);
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
        loudness: meter.push(z),
        correlation: correlationOf(layout, rms),
      };
    };

    const buses: BusMeter[] = [];
    for (const b of this.shape.buses) {
      const contributions = this.shape.channels
        .filter((c) => state.channels[c.id]!.routes[b.id])
        .map((c) => ({ id: c.id, gain: post(c.id), pan: state.channels[c.id]!.pan.lr, fr: state.channels[c.id]!.pan.fr, surround: state.channels[c.id]!.pan }));
      const st = state.buses[b.id]!;
      buses.push(busMeter(b.id, b.layout, mixInto(b.layout, contributions), st.fader, st.mute, st.limiter));
    }
    const auxes: BusMeter[] = [];
    for (const a of this.shape.auxes) {
      const contributions = this.shape.channels.flatMap((c) => {
        const send = state.channels[c.id]!.sends[a.id];
        if (!send?.on) return [];
        const base = send.pre ? 1 : post(c.id);
        return [{ id: c.id, gain: base * dbToGain(send.level), pan: send.pan, fr: 0 }];
      });
      const st = state.auxes[a.id]!;
      auxes.push(busMeter(a.id, a.layout, mixInto(a.layout, contributions), st.fader, st.mute, st.limiter));
    }
    const mixMinus: BusMeter[] = [];
    for (const mm of this.shape.mixMinus) {
      const st = state.mixMinus[mm.id]!;
      const contributions = this.shape.channels
        .filter((c) => state.channels[c.id]!.routes[mm.bus] && !st.exclude.includes(c.id))
        .map((c) => ({ id: c.id, gain: post(c.id), pan: state.channels[c.id]!.pan.lr, fr: state.channels[c.id]!.pan.fr, surround: state.channels[c.id]!.pan }));
      mixMinus.push(busMeter(mm.id, mm.layout, mixInto(mm.layout, contributions), st.fader, st.mute));
    }
    return { channels, buses, auxes, mixMinus };
  }
}
