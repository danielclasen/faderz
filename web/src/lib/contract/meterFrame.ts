// Codec of the binary meter frame, api/ws/server-message.schema.json
// #/$defs/MeterFrame. Pure: no DOM or Node APIs, so it runs in a page, a
// Web Worker and Node alike.

import { METER_FRAME_MAGIC, METER_FRAME_VERSION, METER_MAX, METER_MIN, METER_NULL } from './protocol.ts';
import type { BusMeter, ChannelMeter, Id, Loudness, MonitorMeter, Shape } from './types';

/** Header bytes before the replica id. */
const FIXED_HEADER = 12;
const MAX_REPLICA_LENGTH = 32;
const FNV_OFFSET = 2166136261;
const FNV_PRIME = 16777619;
/** Loudness values (momentary, shortTerm, integrated, lra, maxMomentary, maxShortTerm), correlation and flags per bus. */
const BUS_TAIL = 8;
const SILENCE_DB = -120;
const MAX_DB = 30;
const MAX_LRA = 100;
const MAX_CORRELATION = 1;

export interface MeterElement {
  readonly id: Id;
  /** Meter legs of the element: its width. */
  readonly legs: number;
}

/** Order and size of the values in a meter frame, derived from a shape. */
export interface MeterLayout {
  readonly channels: readonly MeterElement[];
  readonly buses: readonly MeterElement[];
  readonly auxes: readonly MeterElement[];
  readonly mixMinus: readonly MeterElement[];
  readonly monitor: readonly MeterElement[];
  /** FNV-1a 32-bit of the layout text; the frame header carries it. */
  readonly fingerprint: number;
  /** Int16 values per frame. */
  readonly valueCount: number;
}

export interface MeterFrameHeader {
  /** Meter period counter of the replica, uint32. */
  sequence: number;
  /** Id of the sending replica. */
  replica: Id;
}

/** Meter values in shape order, as model.schema.json meter objects. */
export interface MeterValues {
  channels: ChannelMeter[];
  buses: BusMeter[];
  auxes: BusMeter[];
  mixMinus: BusMeter[];
  monitor: MonitorMeter[];
}

export type MeterFrame = MeterFrameHeader & MeterValues;

export type MeterFrameErrorReason = 'short' | 'magic' | 'version' | 'replica' | 'fingerprint' | 'length';

/**
 * Thrown by decodeMeterFrame for a frame that does not fit the layout.
 * reason 'fingerprint' means the replica runs with another shape than the
 * one the layout was derived from.
 */
export class MeterFrameError extends Error {
  readonly reason: MeterFrameErrorReason;

  constructor(reason: MeterFrameErrorReason, message: string) {
    super(message);
    this.name = 'MeterFrameError';
    this.reason = reason;
  }
}

type MeteredShape = Pick<Shape, 'channels' | 'buses' | 'auxes' | 'mixMinus' | 'monitorOutputs'>;

/** Layout of the meter frames of a replica running with this shape. */
export function deriveMeterLayout(shape: MeteredShape): MeterLayout {
  const elements = (list: ReadonlyArray<{ id: Id; width: number }>) => list.map((e) => ({ id: e.id, legs: e.width }));
  const channels = elements(shape.channels);
  const buses = elements(shape.buses);
  const auxes = elements(shape.auxes);
  const mixMinus = elements(shape.mixMinus);
  const monitor = elements(shape.monitorOutputs);

  let text = '';
  let valueCount = 0;
  for (const c of channels) {
    text += `c/${c.id}/${c.legs};`;
    valueCount += 2 * c.legs + 1;
  }
  for (const [section, list] of [['b', buses], ['a', auxes], ['m', mixMinus]] as const) {
    for (const e of list) {
      text += `${section}/${e.id}/${e.legs};`;
      valueCount += 3 * e.legs + BUS_TAIL;
    }
  }
  for (const e of monitor) {
    text += `o/${e.id}/${e.legs};`;
    valueCount += e.legs;
  }
  return { channels, buses, auxes, mixMinus, monitor, fingerprint: fnv1a(text), valueCount };
}

/** FNV-1a 32-bit over the char codes of an ASCII string. */
export function fnv1a(text: string): number {
  let h = FNV_OFFSET;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), FNV_PRIME) >>> 0;
  return h;
}

/**
 * dB (or LUFS, LU) to the Int16 wire value: floor(dB * 100 + 0.5) clamped
 * to METER_MIN..METER_MAX. null, NaN and -Infinity give METER_NULL.
 */
export function toCentiDb(db: number | null): number {
  if (db === null || Number.isNaN(db) || db === -Infinity) return METER_NULL;
  return Math.min(METER_MAX, Math.max(METER_MIN, Math.floor(db * 100 + 0.5)));
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

function headerLength(replicaLength: number): number {
  return FIXED_HEADER + replicaLength + (replicaLength & 1);
}

/**
 * Encodes one meter frame. values must hold one meter per layout element,
 * in layout order, each leg array as long as the element's legs; anything
 * else throws a RangeError, as does a replica id that is not 1 to 32 ASCII
 * characters. sequence is written modulo 2^32.
 */
export function encodeMeterFrame(layout: MeterLayout, header: MeterFrameHeader, values: MeterValues): Uint8Array {
  const { replica } = header;
  if (replica.length < 1 || replica.length > MAX_REPLICA_LENGTH || !/^[\x20-\x7e]+$/.test(replica)) {
    throw new RangeError(`replica id must be 1 to ${MAX_REPLICA_LENGTH} ASCII characters: ${JSON.stringify(replica)}`);
  }
  const start = headerLength(replica.length);
  const bytes = new Uint8Array(start + 2 * layout.valueCount);
  const view = new DataView(bytes.buffer);
  view.setUint8(0, METER_FRAME_MAGIC);
  view.setUint8(1, METER_FRAME_VERSION);
  view.setUint8(2, replica.length);
  view.setUint32(4, header.sequence >>> 0, true);
  view.setUint32(8, layout.fingerprint, true);
  for (let i = 0; i < replica.length; i++) bytes[FIXED_HEADER + i] = replica.charCodeAt(i);

  let o = start;
  const put = (db: number | null) => {
    view.setInt16(o, toCentiDb(db), true);
    o += 2;
  };
  const legs = (what: string, list: readonly number[], n: number) => {
    if (list.length !== n) throw new RangeError(`${what}: ${list.length} legs, layout has ${n}`);
    // A silent leg is the floor, not null: it was measured.
    for (const v of list) put(v === -Infinity ? SILENCE_DB : v);
  };
  const section = <T>(name: string, elements: readonly MeterElement[], list: readonly T[], write: (m: T, e: MeterElement) => void) => {
    if (list.length !== elements.length) {
      throw new RangeError(`${name}: ${list.length} meters, layout has ${elements.length}`);
    }
    elements.forEach((e, i) => write(list[i]!, e));
  };
  const bus = (m: BusMeter, e: MeterElement) => {
    legs(`${e.id} peak`, m.peak, e.legs);
    legs(`${e.id} truePeak`, m.truePeak, e.legs);
    legs(`${e.id} rms`, m.rms, e.legs);
    put(m.loudness.momentary);
    put(m.loudness.shortTerm);
    put(m.loudness.integrated);
    put(m.loudness.lra);
    put(m.loudness.maxMomentary);
    put(m.loudness.maxShortTerm);
    put(m.correlation);
    view.setInt16(o, (m.loudness.integratedValid ? 1 : 0) | (m.loudness.lraStable ? 2 : 0), true);
    o += 2;
  };
  section('channels', layout.channels, values.channels, (m, e) => {
    legs(`${e.id} peak`, m.peak, e.legs);
    legs(`${e.id} truePeak`, m.truePeak, e.legs);
    put(m.momentary);
  });
  section('buses', layout.buses, values.buses, bus);
  section('auxes', layout.auxes, values.auxes, bus);
  section('mixMinus', layout.mixMinus, values.mixMinus, bus);
  section('monitor', layout.monitor, values.monitor, (m, e) => legs(`${e.id} peak`, m.peak, e.legs));
  return bytes;
}

/**
 * Decodes one meter frame against the layout derived from the client's
 * shape. Throws MeterFrameError when magic, version, replica id length,
 * fingerprint or byte length do not match. The result validates against
 * model.schema.json: legs are clamped to -120..30 dB with METER_NULL read
 * as -120, loudness to -120..30 LUFS, LRA to 0..100 LU and correlation
 * to -1..1, with METER_NULL read as null.
 */
export function decodeMeterFrame(layout: MeterLayout, data: ArrayBuffer | ArrayBufferView): MeterFrame {
  const view = ArrayBuffer.isView(data)
    ? new DataView(data.buffer, data.byteOffset, data.byteLength)
    : new DataView(data);
  const size = view.byteLength;
  if (size < FIXED_HEADER) throw new MeterFrameError('short', `meter frame of ${size} bytes`);
  if (view.getUint8(0) !== METER_FRAME_MAGIC) throw new MeterFrameError('magic', `magic ${view.getUint8(0)}`);
  if (view.getUint8(1) !== METER_FRAME_VERSION) throw new MeterFrameError('version', `version ${view.getUint8(1)}`);
  const replicaLength = view.getUint8(2);
  if (replicaLength < 1 || replicaLength > MAX_REPLICA_LENGTH) {
    throw new MeterFrameError('replica', `replica id length ${replicaLength}`);
  }
  const start = headerLength(replicaLength);
  if (size < start) throw new MeterFrameError('short', `meter frame of ${size} bytes, header needs ${start}`);
  const fingerprint = view.getUint32(8, true);
  if (fingerprint !== layout.fingerprint) {
    throw new MeterFrameError('fingerprint', `fingerprint ${fingerprint}, layout has ${layout.fingerprint}`);
  }
  if (size !== start + 2 * layout.valueCount) {
    throw new MeterFrameError('length', `meter frame of ${size} bytes, layout needs ${start + 2 * layout.valueCount}`);
  }
  let replica = '';
  for (let i = 0; i < replicaLength; i++) {
    const c = view.getUint8(FIXED_HEADER + i);
    if (c < 0x20 || c > 0x7e) throw new MeterFrameError('replica', `replica id byte ${c}`);
    replica += String.fromCharCode(c);
  }

  let o = start;
  const next = () => {
    const v = view.getInt16(o, true);
    o += 2;
    return v;
  };
  const leg = () => {
    const v = next();
    return v === METER_NULL ? SILENCE_DB : clamp(v / 100, SILENCE_DB, MAX_DB);
  };
  const legs = (n: number) => {
    const out = new Array<number>(n);
    for (let i = 0; i < n; i++) out[i] = leg();
    return out;
  };
  const optional = (lo: number, hi: number) => {
    const v = next();
    return v === METER_NULL ? null : clamp(v / 100, lo, hi);
  };
  const bus = (e: MeterElement): BusMeter => {
    const peak = legs(e.legs);
    const truePeak = legs(e.legs);
    const rms = legs(e.legs);
    const momentary = optional(SILENCE_DB, MAX_DB);
    const shortTerm = optional(SILENCE_DB, MAX_DB);
    const integrated = optional(SILENCE_DB, MAX_DB);
    const lra = optional(0, MAX_LRA);
    const maxMomentary = optional(SILENCE_DB, MAX_DB);
    const maxShortTerm = optional(SILENCE_DB, MAX_DB);
    const correlation = optional(-MAX_CORRELATION, MAX_CORRELATION);
    const flags = next();
    const loudness: Loudness = {
      momentary,
      shortTerm,
      integrated,
      lra,
      maxMomentary,
      maxShortTerm,
      integratedValid: (flags & 1) === 1,
      lraStable: (flags & 2) === 2,
    };
    return { peak, truePeak, rms, loudness, correlation };
  };
  const channels = layout.channels.map((e): ChannelMeter => {
    const peak = legs(e.legs);
    const truePeak = legs(e.legs);
    return { peak, truePeak, momentary: optional(SILENCE_DB, MAX_DB) };
  });
  const buses = layout.buses.map(bus);
  const auxes = layout.auxes.map(bus);
  const mixMinus = layout.mixMinus.map(bus);
  const monitor = layout.monitor.map((e): MonitorMeter => ({ peak: legs(e.legs) }));
  return { sequence: view.getUint32(4, true), replica, channels, buses, auxes, mixMinus, monitor };
}
