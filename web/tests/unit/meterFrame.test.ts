import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  decodeMeterFrame,
  deriveMeterLayout,
  encodeMeterFrame,
  fnv1a,
  MeterFrameError,
  toCentiDb,
  type MeterLayout,
  type MeterValues,
} from '../../src/lib/contract/meterFrame';
import { METER_FRAME_MAGIC, METER_FRAME_VERSION, METER_NULL, METER_SILENCE } from '../../src/lib/contract/protocol';
import type { BusMeter, ChannelMeter, Layout, Shape, Width } from '../../src/lib/contract/types';
import { API_DIR, Contract } from '../../dev/mock/contract';

const LAYOUT_OF: Record<Width, Layout> = { 1: 'mono', 2: 'stereo', 6: '5.1', 8: '7.1' };
const el = (id: string, width: Width) => ({ id, name: id, width, layout: LAYOUT_OF[width] });

function shape(channels: Width[], buses: Width[], auxes: Width[], mixMinus: Width[], monitor: Width[] = []): Shape {
  return {
    sampleRate: 48000,
    frameRate: { numerator: 25, denominator: 1 },
    channels: channels.map((w, i) => ({ ...el(`ch${i + 1}`, w), altInput: false })),
    buses: buses.map((w, i) => ({ ...el(`bus${i + 1}`, w), kind: 'program' as const })),
    auxes: auxes.map((w, i) => el(`aux${i + 1}`, w)),
    mixMinus: mixMinus.map((w, i) => ({ ...el(`mm${i + 1}`, w), bus: 'bus1' })),
    vcas: [],
    monitorOutputs: monitor.map((w, i) => el(`mon${i + 1}`, w)),
    outputs: [],
  };
}

/** Deterministic values on the 0.01 dB grid, inside the model ranges. */
function values(layout: MeterLayout, seed = 1): MeterValues {
  let s = seed;
  const rnd = () => {
    s = (Math.imul(s, 1103515245) + 12345) >>> 0;
    return s / 2 ** 32;
  };
  const grid = (lo: number, hi: number) => Math.round(lo * 100 + rnd() * (hi - lo) * 100) / 100;
  const legs = (n: number) => Array.from({ length: n }, () => grid(-120, 30));
  const maybe = (lo: number, hi: number) => (rnd() < 0.2 ? null : grid(lo, hi));
  const bus = (e: { legs: number }): BusMeter => ({
    peak: legs(e.legs),
    truePeak: legs(e.legs),
    rms: legs(e.legs),
    loudness: {
      momentary: maybe(-120, 30),
      shortTerm: maybe(-120, 30),
      integrated: maybe(-120, 30),
      lra: maybe(0, 100),
      maxMomentary: maybe(-120, 30),
      maxShortTerm: maybe(-120, 30),
      integratedValid: rnd() < 0.5,
      lraStable: rnd() < 0.5,
    },
    correlation: maybe(-1, 1),
  });
  return {
    channels: layout.channels.map((e): ChannelMeter => ({ peak: legs(e.legs), truePeak: legs(e.legs), momentary: maybe(-120, 30) })),
    buses: layout.buses.map(bus),
    auxes: layout.auxes.map(bus),
    mixMinus: layout.mixMinus.map(bus),
    monitor: layout.monitor.map((e) => ({ peak: legs(e.legs) })),
  };
}

const contract = new Contract();
const MODEL = 'https://github.com/danielclasen/faderz/raw/main/api/ws/model.schema.json';
const channelMeter = contract.ajv.compile({ $ref: `${MODEL}#/$defs/ChannelMeter` });
const busMeter = contract.ajv.compile({ $ref: `${MODEL}#/$defs/BusMeter` });
const monitorMeter = contract.ajv.compile({ $ref: `${MODEL}#/$defs/MonitorMeter` });

function expectModelValid(v: MeterValues): void {
  for (const m of v.channels) expect(channelMeter(m), JSON.stringify(channelMeter.errors)).toBe(true);
  for (const m of [...v.buses, ...v.auxes, ...v.mixMinus]) expect(busMeter(m), JSON.stringify(busMeter.errors)).toBe(true);
  for (const m of v.monitor) expect(monitorMeter(m), JSON.stringify(monitorMeter.errors)).toBe(true);
}

// One mono channel ch1 and one stereo bus pgm, replica "rep" (odd length,
// so one padding byte), sequence 258.
const TINY = deriveMeterLayout({
  channels: [{ ...el('ch1', 1), altInput: false }],
  buses: [{ ...el('pgm', 2), kind: 'program' }],
  auxes: [],
  mixMinus: [],
  monitorOutputs: [],
});
const TINY_VALUES: MeterValues = {
  channels: [{ peak: [-6.5], truePeak: [-5.9], momentary: -20.004 }],
  buses: [
    {
      peak: [0, 3.21],
      truePeak: [0.5, -Infinity],
      rms: [-18, -18.5],
      loudness: {
        momentary: -23,
        shortTerm: null,
        integrated: -23.456,
        lra: 7.25,
        maxMomentary: -21.5,
        maxShortTerm: null,
        integratedValid: true,
        lraStable: false,
      },
      correlation: 0.85,
    },
  ],
  auxes: [],
  mixMinus: [],
  monitor: [],
};
// prettier-ignore
const TINY_BYTES = [
  0x4d, 0x03, 0x03, 0x00, // magic 'M', version 3, replica id length 3, reserved
  0x02, 0x01, 0x00, 0x00, // sequence 258
  0xf5, 0x1d, 0xdd, 0xbd, // fingerprint FNV-1a("c/ch1/1;b/pgm/2;") = 0xbddd1df5
  0x72, 0x65, 0x70, 0x00, // "rep", padding
  0x76, 0xfd, // ch1 peak -6.5 -> -650
  0xb2, 0xfd, // ch1 truePeak -5.9 -> -590
  0x30, 0xf8, // ch1 momentary -20.004 -> -2000
  0x00, 0x00, 0x41, 0x01, // pgm peak 0, 3.21 -> 0, 321
  0x32, 0x00, 0x20, 0xd1, // pgm truePeak 0.5, -Infinity -> 50, -12000
  0xf8, 0xf8, 0xc6, 0xf8, // pgm rms -18, -18.5 -> -1800, -1850
  0x04, 0xf7, // momentary -23 -> -2300
  0x00, 0x80, // shortTerm null -> -32768
  0xd6, 0xf6, // integrated -23.456 -> -2346
  0xd5, 0x02, // lra 7.25 -> 725
  0x9a, 0xf7, // maxMomentary -21.5 -> -2150
  0x00, 0x80, // maxShortTerm null -> -32768
  0x55, 0x00, // correlation 0.85 -> 85
  0x01, 0x00, // flags: integratedValid, not lraStable
];

describe('meter frame byte layout', () => {
  it('matches the hand-written bytes of the documented layout', () => {
    const bytes = encodeMeterFrame(TINY, { sequence: 258, replica: 'rep' }, TINY_VALUES);
    expect([...bytes]).toEqual(TINY_BYTES);
  });

  it('matches the example of the MeterFrame schema', () => {
    const schema = JSON.parse(readFileSync(join(API_DIR, 'ws', 'server-message.schema.json'), 'utf8')) as {
      $defs: { MeterFrame: { examples: string[] } };
    };
    const hex = schema.$defs.MeterFrame.examples[0]!.split(' ').map((b) => parseInt(b, 16));
    expect(hex).toEqual(TINY_BYTES);
  });

  it('decodes the hand-written bytes into model meter objects', () => {
    const frame = decodeMeterFrame(TINY, new Uint8Array(TINY_BYTES));
    expect(frame).toEqual({
      sequence: 258,
      replica: 'rep',
      channels: [{ peak: [-6.5], truePeak: [-5.9], momentary: -20 }],
      buses: [
        {
          peak: [0, 3.21],
          truePeak: [0.5, -120],
          rms: [-18, -18.5],
          loudness: {
            momentary: -23,
            shortTerm: null,
            integrated: -23.46,
            lra: 7.25,
            maxMomentary: -21.5,
            maxShortTerm: null,
            integratedValid: true,
            lraStable: false,
          },
          correlation: 0.85,
        },
      ],
      auxes: [],
      mixMinus: [],
      monitor: [],
    });
    expectModelValid(frame);
  });

  it('starts the values at an even offset without padding for an even replica id', () => {
    const bytes = encodeMeterFrame(TINY, { sequence: 0, replica: 'r1' }, TINY_VALUES);
    expect(bytes.length).toBe(12 + 2 + 2 * 17);
    expect([...bytes.slice(12, 16)]).toEqual([0x72, 0x31, 0x76, 0xfd]);
  });
});

describe('meter layout', () => {
  it('orders channels, buses, auxes, mix-minus and monitor outputs with their legs', () => {
    const layout = deriveMeterLayout(shape([1, 2, 6, 8], [2], [1], [2], [2]));
    expect(layout.channels.map((e) => e.legs)).toEqual([1, 2, 6, 8]);
    expect(layout.valueCount).toBe(2 * 17 + 4 + (3 * 2 + 8) + (3 * 1 + 8) + (3 * 2 + 8) + 2);
    expect(layout.fingerprint).toBe(
      fnv1a('c/ch1/1;c/ch2/2;c/ch3/6;c/ch4/8;b/bus1/2;a/aux1/1;m/mm1/2;o/mon1/2;'),
    );
  });

  it('uses FNV-1a 32-bit', () => {
    expect(fnv1a('')).toBe(0x811c9dc5);
    expect(fnv1a('a')).toBe(0xe40c292c);
    expect(fnv1a('foobar')).toBe(0xbf9cf968);
  });
});

describe('meter frame round trip', () => {
  it.each([
    ['empty', shape([], [], [], [])],
    ['one of each', shape([1], [2], [2], [1], [2])],
    ['every width', shape([1, 2, 6, 8], [2, 6, 8], [1, 2], [2, 2])],
    ['64 channels', shape(Array<Width>(64).fill(2), [2, 6, 2], Array<Width>(8).fill(2), Array<Width>(4).fill(2), [6, 2])],
  ])('%s', (_, s) => {
    const layout = deriveMeterLayout(s);
    for (const seed of [1, 2, 3]) {
      const v = values(layout, seed);
      const bytes = encodeMeterFrame(layout, { sequence: seed * 1000, replica: 'r2' }, v);
      const frame = decodeMeterFrame(layout, bytes);
      expect(frame).toEqual({ sequence: seed * 1000, replica: 'r2', ...v });
      expectModelValid(frame);
    }
  });

  it('decodes from a view into a larger buffer and from an ArrayBuffer', () => {
    const bytes = encodeMeterFrame(TINY, { sequence: 7, replica: 'rep' }, TINY_VALUES);
    const outer = new Uint8Array(bytes.length + 5);
    outer.set(bytes, 3);
    const viaView = decodeMeterFrame(TINY, outer.subarray(3, 3 + bytes.length));
    const viaBuffer = decodeMeterFrame(TINY, bytes.slice().buffer);
    expect(viaView).toEqual(viaBuffer);
    expect(viaView.sequence).toBe(7);
  });

  it('writes sequence modulo 2^32', () => {
    for (const [given, read] of [
      [0xffffffff, 0xffffffff],
      [2 ** 32 + 5, 5],
    ]) {
      expect(decodeMeterFrame(TINY, encodeMeterFrame(TINY, { sequence: given!, replica: 'rep' }, TINY_VALUES)).sequence).toBe(read);
    }
  });
});

describe('meter values', () => {
  const one = deriveMeterLayout(shape([], [1], [], []));
  const busWith = (leg: number, loudness: Partial<BusMeter['loudness']>): MeterValues => ({
    channels: [],
    buses: [
      {
        peak: [leg],
        truePeak: [leg],
        rms: [leg],
        loudness: {
          momentary: null,
          shortTerm: null,
          integrated: null,
          lra: null,
          maxMomentary: null,
          maxShortTerm: null,
          integratedValid: false,
          lraStable: false,
          ...loudness,
        },
        correlation: null,
      },
    ],
    auxes: [],
    mixMinus: [],
    monitor: [],
  });
  const wire = (bytes: Uint8Array) => {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return Array.from({ length: one.valueCount }, (_, i) => view.getInt16(14 + 2 * i, true));
  };

  it('quantises to 0.01 dB, rounding halves up', () => {
    expect(toCentiDb(-6.004)).toBe(-600);
    expect(toCentiDb(-6.006)).toBe(-601);
    expect(toCentiDb(0.125)).toBe(13);
    expect(toCentiDb(-0.125)).toBe(-12);
    expect(toCentiDb(-3.21)).toBe(-321);
    expect(decodeMeterFrame(one, encodeMeterFrame(one, { sequence: 0, replica: 'r1' }, busWith(-12.3456, {}))).buses[0]!.peak).toEqual([-12.35]);
  });

  it('writes the floor for a silent leg, the null sentinel for null and NaN', () => {
    const bytes = encodeMeterFrame(one, { sequence: 0, replica: 'r1' }, busWith(-Infinity, { lra: Number.NaN }));
    expect(wire(bytes)).toEqual([...Array<number>(3).fill(METER_SILENCE), ...Array<number>(7).fill(METER_NULL), 0]);
    const bus = decodeMeterFrame(one, bytes).buses[0]!;
    expect(bus.peak).toEqual([-120]);
    expect(bus.loudness).toEqual({
      momentary: null,
      shortTerm: null,
      integrated: null,
      lra: null,
      maxMomentary: null,
      maxShortTerm: null,
      integratedValid: false,
      lraStable: false,
    });
    expect(bus.correlation).toBeNull();
    expectModelValid(decodeMeterFrame(one, bytes));
  });

  it('clamps to the Int16 range on the wire and to the model range when decoding', () => {
    const bytes = encodeMeterFrame(one, { sequence: 0, replica: 'r1' }, busWith(400, { momentary: -400, shortTerm: Infinity, integrated: 31, lra: 150 }));
    expect(wire(bytes)).toEqual([32767, 32767, 32767, -32767, 32767, 3100, 15000, METER_NULL, METER_NULL, METER_NULL, 0]);
    const frame = decodeMeterFrame(one, bytes);
    expect(frame.buses[0]).toEqual({
      peak: [30],
      truePeak: [30],
      rms: [30],
      loudness: { momentary: -120, shortTerm: 30, integrated: 30, lra: 100, maxMomentary: null, maxShortTerm: null, integratedValid: false, lraStable: false },
      correlation: null,
    });
    expectModelValid(frame);
    const low = decodeMeterFrame(one, encodeMeterFrame(one, { sequence: 0, replica: 'r1' }, busWith(-400, { lra: -3 })));
    expect(low.buses[0]!.peak).toEqual([-120]);
    expect(low.buses[0]!.loudness.lra).toBe(0);
  });

  it('refuses values that do not fit the layout', () => {
    const enc = (v: MeterValues, replica = 'r1') => () => encodeMeterFrame(one, { sequence: 0, replica }, v);
    expect(enc({ ...busWith(0, {}), buses: [] })).toThrow(RangeError);
    expect(enc({ ...busWith(0, {}), buses: [{ ...busWith(0, {}).buses[0]!, rms: [0, 0] }] })).toThrow(RangeError);
    expect(enc(busWith(0, {}), '')).toThrow(RangeError);
    expect(enc(busWith(0, {}), 'r'.repeat(33))).toThrow(RangeError);
  });
});

describe('meter frame header checks', () => {
  const frame = () => encodeMeterFrame(TINY, { sequence: 1, replica: 'rep' }, TINY_VALUES);
  const reason = (layout: MeterLayout, data: Uint8Array) => {
    try {
      decodeMeterFrame(layout, data);
    } catch (e) {
      expect(e).toBeInstanceOf(MeterFrameError);
      return (e as MeterFrameError).reason;
    }
    return 'decoded';
  };
  const patched = (offset: number, byte: number) => {
    const b = frame();
    b[offset] = byte;
    return b;
  };

  it('rejects a wrong magic, version or replica id', () => {
    expect(reason(TINY, patched(0, METER_FRAME_MAGIC + 1))).toBe('magic');
    expect(reason(TINY, patched(1, METER_FRAME_VERSION + 1))).toBe('version');
    expect(reason(TINY, patched(2, 0))).toBe('replica');
    expect(reason(TINY, patched(2, 33))).toBe('replica');
    expect(reason(TINY, patched(13, 0x07))).toBe('replica');
  });

  it('rejects a frame of another shape with the same length', () => {
    const a = deriveMeterLayout(shape([2, 1], [], [], []));
    const b = deriveMeterLayout(shape([1, 2], [], [], []));
    expect(a.valueCount).toBe(b.valueCount);
    const bytes = encodeMeterFrame(a, { sequence: 1, replica: 'r1' }, values(a));
    expect(reason(a, bytes)).toBe('decoded');
    expect(reason(b, bytes)).toBe('fingerprint');
  });

  it('rejects a truncated, extended or short frame', () => {
    const b = frame();
    expect(reason(TINY, b.subarray(0, b.length - 2))).toBe('length');
    const longer = new Uint8Array(b.length + 2);
    longer.set(b);
    expect(reason(TINY, longer)).toBe('length');
    expect(reason(TINY, b.subarray(0, 11))).toBe('short');
    expect(reason(TINY, b.subarray(0, 13))).toBe('short');
  });
});
