// Example shape and starting state of the dev-only mock server.

import type {
  BusState,
  ChannelShape,
  ChannelState,
  ConsoleStatus,
  Eq,
  ElementShape,
  Shape,
  State,
} from '../../src/lib/contract/types.ts';
import { handleOf } from './handle.ts';

const MONO_NAMES = [
  'Pres 1', 'Pres 2', 'Guest 1', 'Guest 2', 'Guest 3', 'Guest 4',
  'Rpt 1', 'Reporter 2 North Gate', 'Rpt 3', 'Rpt 4', 'Phone 1', 'Phone 2',
  'Amb 1', 'Amb 2', 'Amb 3', 'Amb 4', 'Lav 1', 'Lav 2',
  'Lav 3', 'Lav 4', 'Hand 1', 'Hand 2', 'Spot 1', 'Spot 2',
];

const WIDE_CHANNELS: ChannelShape[] = [
  { id: 'st1', name: 'Music', width: 2, layout: 'stereo', altInput: false },
  { id: 'st2', name: 'VT 1', width: 2, layout: 'stereo', altInput: false },
  { id: 'st3', name: 'VT 2', width: 2, layout: 'stereo', altInput: false },
  { id: 'st4', name: 'FX', width: 2, layout: 'stereo', altInput: false },
  { id: 'sur1', name: 'Film 5.1', width: 6, layout: '5.1', altInput: true },
];

/** Channel count of the example shape when none is given. */
export const DEFAULT_CHANNELS = MONO_NAMES.length + WIDE_CHANNELS.length;

/**
 * Example shape with `channelCount` input channels: mono channels ch1 to
 * chN, then the stereo and 5.1 channels st1 to st4 and sur1, as many of
 * those as fit.
 */
export function exampleShape(channelCount = DEFAULT_CHANNELS): Shape {
  if (!Number.isInteger(channelCount) || channelCount < 1) {
    throw new RangeError(`channel count must be a positive integer: ${channelCount}`);
  }
  const mono = Math.max(0, channelCount - WIDE_CHANNELS.length);
  // The presenter microphones have a redundant B input.
  const channels: ChannelShape[] = [
    ...Array.from({ length: mono }, (_, i) => ({
      id: `ch${i + 1}`,
      name: MONO_NAMES[i] ?? `Input ${i + 1}`,
      width: 1 as const,
      layout: 'mono' as const,
      altInput: i < 2,
    })),
    ...WIDE_CHANNELS.slice(0, channelCount - mono).map((c) => ({ ...c })),
  ];
  const buses: Shape['buses'] = [
    { id: 'pgm51', name: 'PGM 5.1', kind: 'program', width: 6, layout: '5.1' },
    { id: 'pgm20', name: 'PGM 2.0', kind: 'program', width: 2, layout: 'stereo' },
    { id: 'me', name: 'M&E', kind: 'me', width: 2, layout: 'stereo' },
    { id: 'cf', name: 'Clean', kind: 'cleanFeed', width: 2, layout: 'stereo' },
  ];
  const auxes: ElementShape[] = [
    { id: 'aux1', name: 'IFB 1', width: 1, layout: 'mono' },
    { id: 'aux2', name: 'IFB 2', width: 1, layout: 'mono' },
    { id: 'aux3', name: 'Stage', width: 2, layout: 'stereo' },
    { id: 'aux4', name: 'Rec', width: 2, layout: 'stereo' },
  ];
  const mixMinus: Shape['mixMinus'] = [
    { id: 'mm1', name: 'N-1 Phone 1', bus: 'pgm20', width: 2, layout: 'stereo' },
    { id: 'mm2', name: 'N-1 Phone 2', bus: 'pgm20', width: 2, layout: 'stereo' },
  ];
  const monitorOutputs: ElementShape[] = [{ id: 'mon', name: 'Monitor', width: 2, layout: 'stereo' }];
  // Direct outs of the presenter channels, e.g. for an ISO recorder.
  const directOuts = channels.filter((c) => c.id === 'ch1' || c.id === 'ch2');
  const outputs: Shape['outputs'] = [...buses, ...auxes, ...mixMinus, ...monitorOutputs, ...directOuts].map((e) => ({
    id: `out-${e.id}`,
    name: `${e.name} out`,
    source: e.id,
    width: e.width,
    layout: e.layout,
  }));
  return {
    sampleRate: 48000,
    frameRate: { numerator: 25, denominator: 1 },
    channels,
    buses,
    auxes,
    mixMinus,
    vcas: [
      { id: 'vca1', name: 'PRES' },
      { id: 'vca2', name: 'GUEST PANEL' },
    ],
    monitorOutputs,
    outputs,
  };
}

function flatEq(): Eq {
  return {
    enabled: false,
    bands: [
      { type: 'lowShelf', freq: 100, gain: 0, q: 0.7 },
      { type: 'peak', freq: 400, gain: 0, q: 1 },
      { type: 'peak', freq: 2500, gain: 0, q: 1 },
      { type: 'highShelf', freq: 8000, gain: 0, q: 0.7 },
    ],
  };
}

function busState(width: number, fader = 0, limiter = false): BusState {
  return {
    polarity: Array<boolean>(width).fill(false),
    eq: flatEq(),
    gate: { enabled: false, threshold: -60, range: 20, attack: 1, release: 200 },
    comp: { enabled: false, threshold: -12, ratio: 2, attack: 10, release: 200, makeup: 0 },
    dynamicsBeforeEq: false,
    limiter: { enabled: limiter, ceiling: -1 },
    delay: 0,
    fader,
    mute: false,
  };
}

/** Session id each console uses as a client; the state carries its handle (ConsoleStatus.handle). */
export const CONSOLE_SESSIONS: Record<string, string> = { 'mcu-1': 'console-mcu-1-Q2xQnS8fHw', 'ext-1': 'console-ext-1-4fVd9KkQ2w', 'osc-1': 'console-osc-1-Zr0aLm3Pxg' };

const FADERS: Record<string, number> = {
  ch1: 0, ch2: -2.5, ch3: -4, ch4: -3, ch5: -6, ch6: -90, ch7: -5, ch8: -90, ch9: -90, ch10: -90,
  ch11: -3, ch12: -90, ch13: -12, ch14: -14, ch15: -18, ch16: -90, ch17: -4, ch18: -4, ch19: -90,
  ch20: -90, ch21: -6, ch22: -90, ch23: -90, ch24: -90, st1: -12, st2: -90, st3: -90, st4: -8, sur1: -90,
};

export function initialState(shape: Shape): State {
  const channels: State['channels'] = {};
  shape.channels.forEach((c, i) => {
    const mic = c.layout === 'mono';
    const s: ChannelState = {
      input: 'a',
      trim: 0,
      legTrim: Array<number>(c.width).fill(0),
      polarity: Array<boolean>(c.width).fill(false),
      hpf: { enabled: mic, freq: mic ? 80 : 40 },
      eq: flatEq(),
      gate: { enabled: false, threshold: -50, range: 20, attack: 1, release: 200 },
      comp: { enabled: i < 6, threshold: -18, ratio: 3, attack: 10, release: 150, makeup: i < 6 ? 3 : 0 },
      dynamicsBeforeEq: false,
      delay: 0,
      pan: { lr: mic && i >= 2 && i < 6 ? [-0.3, 0.3, -0.15, 0.15][i - 2]! : 0, fr: 0, divergence: 0, lfe: -90 },
      image: 'normal',
      fader: FADERS[c.id] ?? -90,
      mute: c.id === 'ch23' || c.id === 'ch24',
      solo: 'off',
      meterTap: 'preFader',
      directOut: { tap: 'postFader' },
      sends: Object.fromEntries(
        shape.auxes.map((a) => [
          a.id,
          { on: a.id === 'aux4' || (mic && i < 6 && a.id !== 'aux3'), level: a.id === 'aux4' ? 0 : -10, pan: 0, pre: a.id !== 'aux4' },
        ]),
      ),
      routes: Object.fromEntries(
        shape.buses.map((b) => [
          b.id,
          b.kind === 'me' ? !mic || (i >= 12 && i < 16) : b.kind === 'cleanFeed' ? !(i < 2) : true,
        ]),
      ),
    };
    channels[c.id] = s;
  });
  const ch1 = channels['ch1'];
  if (ch1) {
    ch1.eq.enabled = true;
    ch1.eq.bands[1].gain = -3;
    ch1.eq.bands[1].freq = 300;
    ch1.eq.bands[2].gain = 2.5;
    ch1.eq.bands[2].freq = 3200;
  }
  if (channels['ch2']) {
    channels['ch2'].eq.enabled = true;
    channels['ch2'].eq.bands[3].gain = 2;
  }
  if (channels['ch11']) {
    channels['ch11'].eq.enabled = true;
    channels['ch11'].eq.bands[0].gain = -6;
  }
  for (const id of ['ch13', 'ch14']) if (channels[id]) channels[id].gate.enabled = true;
  const existing = (ids: string[]) => ids.filter((id) => channels[id] !== undefined);

  const consoles: ConsoleStatus[] = [
    { id: 'mcu-1', name: 'MCU Pro', handle: handleOf(CONSOLE_SESSIONS['mcu-1']!), protocol: 'mcu', transport: 'bridge', connected: true, bankOffset: 0, strips: 8 },
    { id: 'ext-1', name: 'MCU Extender', handle: handleOf(CONSOLE_SESSIONS['ext-1']!), protocol: 'mcu', transport: 'bridge', connected: true, bankOffset: 8, strips: 8 },
    { id: 'osc-1', name: 'OSC tablet', handle: handleOf(CONSOLE_SESSIONS['osc-1']!), protocol: 'osc', transport: 'osc-udp', connected: false, bankOffset: 0, strips: 16 },
  ];

  return {
    channels,
    buses: Object.fromEntries(shape.buses.map((b) => [b.id, busState(b.width, 0, b.kind === 'program' || b.kind === 'cleanFeed')])),
    auxes: Object.fromEntries(shape.auxes.map((a) => [a.id, busState(a.width, a.id === 'aux3' ? -6 : 0)])),
    mixMinus: Object.fromEntries(
      shape.mixMinus.map((m, i) => [m.id, { exclude: existing([i === 0 ? 'ch11' : 'ch12']), fader: 0, mute: false, interrupt: { source: null, active: false } }]),
    ),
    vcas: {
      vca1: { offset: 0, mute: false, members: existing(['ch1', 'ch2']), solo: 'off' },
      vca2: { offset: -2, mute: false, members: existing(['ch3', 'ch4', 'ch5', 'ch6']), solo: 'off' },
    },
    monitor: {
      source: 'pgm20',
      level: -10,
      dim: false,
      cut: false,
      mono: false,
      talkbackDim: true,
      dimDepth: 20,
      monoCompensation: 3,
      downmix: 'loRo',
      image: 'normal',
      speakerMute: Array<boolean>(shape.monitorOutputs[0]?.width ?? 2).fill(false),
      speakerSolo: Array<boolean>(shape.monitorOutputs[0]?.width ?? 2).fill(false),
    },
    afv: Object.fromEntries(
      shape.channels.map((c, i) => {
        const cam = i >= 12 && i < 16 ? `CAM${i - 11}` : null;
        return [c.id, { enabled: cam !== null, source: cam, riseMs: 40, fallMs: 200, holdMs: 0, offLevel: -90, open: cam === null || cam === 'CAM1' }];
      }),
    ),
    tally: { CAM1: 'program', CAM2: 'preview', CAM3: 'off', CAM4: 'off' },
    recallSafe: Object.fromEntries(existing(['ch11', 'ch12']).map((id) => [id, ['fader', 'mute']])),
    scenes: [],
    sceneRecalled: null,
    consoles,
    // No output takes tone until one is chosen: tone on programme replaces it.
    tone: { on: false, level: -18, ident: false, targets: [] },
    metering: {
      zones: { alignment: -18, permittedMax: -9, truePeakMax: -1 },
      loudness: { preset: 'ebu', target: -23, tolerance: 1 },
      scale: 'ebu9',
    },
  };
}
