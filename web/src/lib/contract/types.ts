// TypeScript view of api/ws/*.schema.json. The contract tests validate
// mock traffic typed with these interfaces against the schemas.

export type Id = string;
export type SessionId = string;
export type SessionKind = 'ui' | 'console' | 'script';
export type LinkMode = 'primary' | 'standby';
export type CommandId = string;
export type Revision = number;
export type Layout = 'mono' | 'stereo' | '5.1' | '7.1';
export type Width = 1 | 2 | 6 | 8;
export type Solo = 'off' | 'pfl' | 'afl';
export type Tap = 'input' | 'preFader' | 'postFader';
export type EqBandType = 'lowShelf' | 'peak' | 'highShelf';
export type TallyState = 'program' | 'preview' | 'off';
export type RecallGroup =
  | 'input'
  | 'eq'
  | 'dynamics'
  | 'pan'
  | 'sends'
  | 'routing'
  | 'fader'
  | 'mute'
  | 'afv';
export type ReplicaRole = 'leader' | 'follower';
export type BusKind = 'program' | 'me' | 'cleanFeed';

export const RECALL_GROUPS: readonly RecallGroup[] = [
  'input',
  'eq',
  'dynamics',
  'pan',
  'sends',
  'routing',
  'fader',
  'mute',
  'afv',
];

export interface ElementShape {
  id: Id;
  name: string;
  width: Width;
  layout: Layout;
}
export type ChannelShape = ElementShape;
export interface BusShape extends ElementShape {
  kind: BusKind;
}
export interface MixMinusShape extends ElementShape {
  bus: Id;
}
export interface OutputShape extends ElementShape {
  source: Id;
}
export interface VcaShape {
  id: Id;
  name: string;
}
export interface FrameRate {
  numerator: number;
  denominator: number;
}
export interface Shape {
  sampleRate: 48000;
  frameRate: FrameRate;
  channels: ChannelShape[];
  buses: BusShape[];
  auxes: ElementShape[];
  mixMinus: MixMinusShape[];
  vcas: VcaShape[];
  monitorOutputs: ElementShape[];
  outputs: OutputShape[];
}

export interface Hpf {
  enabled: boolean;
  freq: number;
}
export interface EqBand {
  type: EqBandType;
  freq: number;
  gain: number;
  q: number;
}
export interface Eq {
  enabled: boolean;
  bands: [EqBand, EqBand, EqBand, EqBand];
}
export interface Gate {
  enabled: boolean;
  threshold: number;
  range: number;
  attack: number;
  release: number;
}
export interface Comp {
  enabled: boolean;
  threshold: number;
  ratio: number;
  attack: number;
  release: number;
  makeup: number;
}
export interface PanState {
  lr: number;
  fr: number;
  /** 0 discrete centre .. 1 phantom centre. */
  divergence: number;
  /** LFE send, dB; -90 off. */
  lfe: number;
}
export type StereoImage = 'normal' | 'leftToBoth' | 'rightToBoth' | 'swap';
export interface Send {
  on: boolean;
  level: number;
  pan: number;
  pre: boolean;
}
export interface ChannelState {
  trim: number;
  /** Per leg, layout order. */
  legTrim: number[];
  /** Per leg, layout order; true inverts. */
  polarity: boolean[];
  hpf: Hpf;
  eq: Eq;
  gate: Gate;
  comp: Comp;
  /** Gate and compressor before EQ. */
  dynamicsBeforeEq: boolean;
  delay: number;
  pan: PanState;
  image: StereoImage;
  fader: number;
  mute: boolean;
  solo: Solo;
  meterTap: Tap;
  directOut: { tap: Tap };
  sends: Record<Id, Send>;
  routes: Record<Id, boolean>;
}
export interface Limiter {
  enabled: boolean;
  /** dBTP. */
  ceiling: number;
}
export interface BusState {
  polarity: boolean[];
  eq: Eq;
  gate: Gate;
  comp: Comp;
  dynamicsBeforeEq: boolean;
  limiter: Limiter;
  delay: number;
  fader: number;
  mute: boolean;
}
export interface Interrupt {
  /** Channel id that talks into the output. */
  source: Id | null;
  /** Momentary: reset by the server when its owner releases it or the lease ends. */
  active: boolean;
}
export interface MixMinusState {
  exclude: Id[];
  fader: number;
  mute: boolean;
  interrupt: Interrupt;
}
export interface VcaState {
  offset: number;
  mute: boolean;
  members: Id[];
}
export interface MonitorState {
  source: Id;
  level: number;
  dim: boolean;
  cut: boolean;
  mono: boolean;
  talkbackDim: boolean;
}
export interface AfvBinding {
  enabled: boolean;
  source: string | null;
  riseMs: number;
  fallMs: number;
  holdMs: number;
  /** Closed level, dB; -90 closes fully. */
  offLevel: number;
  open: boolean;
}
export interface Scene {
  id: Id;
  name: string;
  storedAt: string;
}
export interface ConsoleStatus {
  id: Id;
  name: string;
  session: SessionId;
  protocol: 'mcu' | 'hui' | 'osc';
  transport: 'alsa-midi' | 'bridge' | 'osc-udp';
  connected: boolean;
  bankOffset: number;
  strips: number;
}
export type LoudnessPreset = 'ebu' | 'atsc' | 'custom';
export type LoudnessScale = 'ebu9' | 'ebu18';
export interface Metering {
  zones: { alignment: number; permittedMax: number; truePeakMax: number };
  loudness: { preset: LoudnessPreset; target: number; tolerance: number };
  scale: LoudnessScale;
}
/** Target and tolerance a preset stands for. */
export const LOUDNESS_PRESETS: Record<Exclude<LoudnessPreset, 'custom'>, { target: number; tolerance: number }> = {
  ebu: { target: -23, tolerance: 1 },
  atsc: { target: -24, tolerance: 2 },
};
export interface State {
  channels: Record<Id, ChannelState>;
  buses: Record<Id, BusState>;
  auxes: Record<Id, BusState>;
  mixMinus: Record<Id, MixMinusState>;
  vcas: Record<Id, VcaState>;
  monitor: MonitorState;
  afv: Record<Id, AfvBinding>;
  tally: Record<string, TallyState>;
  recallSafe: Record<Id, RecallGroup[]>;
  scenes: Scene[];
  consoles: ConsoleStatus[];
  metering: Metering;
}
export interface Owner {
  principal: string;
  session: SessionId;
  kind: SessionKind;
  connection: string;
}
export type Owners = Record<string, Owner>;

export interface ReplicaRef {
  id: Id;
  role: ReplicaRole;
}
export interface ReplicaStatus extends ReplicaRef {
  healthy: boolean;
  commitable: boolean;
}
export interface StateDocument {
  revision: Revision;
  replica: ReplicaRef;
  shape: Shape;
  state: State;
  owners: Owners;
}

export interface Change {
  path: string;
  value: unknown;
}
export interface OwnershipChange {
  path: string;
  owner: Owner | null;
}
export interface Conflict {
  path: string;
  current: unknown;
  owner: Owner;
}
export interface Origin {
  owner: Owner;
  commandId: CommandId;
  seq?: number;
}

// Meter values. On the WebSocket they travel in binary meter frames,
// encoded and decoded by meterFrame.ts.
export interface ChannelMeter {
  /** Sample peak at the meter tap. */
  peak: number[];
  /** True peak at the direct out tap. */
  truePeak: number[];
  /** BS.1770 momentary loudness at the meter tap, LUFS. */
  momentary: number | null;
}
export interface Loudness {
  momentary: number | null;
  shortTerm: number | null;
  integrated: number | null;
  lra: number | null;
  maxMomentary: number | null;
  maxShortTerm: number | null;
  integratedValid: boolean;
  lraStable: boolean;
}
export interface BusMeter {
  peak: number[];
  truePeak: number[];
  rms: number[];
  loudness: Loudness;
  /** Phase correlation of a stereo element, -1..1; null otherwise. */
  correlation: number | null;
}

// WebSocket client messages
export interface HelloMessage {
  type: 'hello';
  session: SessionId;
  kind: 'ui' | 'console';
  mode: LinkMode;
  lastRevision?: Revision;
}
export interface PromoteMessage {
  type: 'promote';
  /** Highest revision the client has applied; commands wait until the replica has applied it. */
  lastRevision: Revision;
}
export interface CreditMessage {
  type: 'credit';
  n: number;
}
export interface PingMessage {
  type: 'ping';
  t: number;
}
export interface PongMessage {
  type: 'pong';
  t: number;
}
export interface SetMessage {
  type: 'set';
  id: CommandId;
  seq: number;
  path: string;
  value: unknown;
}
export interface TouchMessage {
  type: 'touch';
  id: CommandId;
  path: string;
}
export interface ReleaseMessage {
  type: 'release';
  id: CommandId;
  path: string;
}
export interface ResyncMessage {
  type: 'resync';
  /** Highest revision the client has applied; the answer waits until the replica has applied it. */
  lastRevision?: Revision;
  /** Answer with a snapshot. */
  full?: true;
}
export type CommandMessage = SetMessage | TouchMessage | ReleaseMessage;
export type ClientMessage =
  | HelloMessage
  | PromoteMessage
  | CommandMessage
  | ResyncMessage
  | CreditMessage
  | PingMessage
  | PongMessage;

// WebSocket server messages (JSON text); meter frames are binary
export interface SnapshotMessage {
  type: 'snapshot';
  you: Owner;
  replica: ReplicaRef;
  revision: Revision;
  shape: Shape;
  state: State;
  owners: Owners;
}
export interface DeltaMessage {
  type: 'delta';
  replica: ReplicaRef;
  revision: Revision;
  atIndex?: number;
  changes?: Change[];
  owners?: OwnershipChange[];
  origins?: Origin[];
}
export interface ResumeMessage {
  type: 'resume';
  you: Owner;
  replica: ReplicaRef;
  revision: Revision;
  deltas: DeltaMessage[];
}
export interface AckMessage {
  type: 'ack';
  replica: ReplicaRef;
  commandId: CommandId;
  result: 'duplicate' | 'stale' | 'noop';
  revision: Revision;
}
export interface ReplicasMessage {
  type: 'replicas';
  replica: ReplicaRef;
  replicas: ReplicaStatus[];
}
export interface CountersMessage {
  type: 'counters';
  replica: ReplicaRef;
  values: Record<string, number>;
}
export interface ErrorMessage {
  type: 'error';
  replica: ReplicaRef;
  code: 400 | 404 | 409;
  message: string;
  commandId?: CommandId;
  path?: string;
  current?: unknown;
  owner?: Owner;
}
export interface ServerPing {
  type: 'ping' | 'pong';
  replica: ReplicaRef;
  t: number;
}
export type ServerMessage =
  | SnapshotMessage
  | ResumeMessage
  | DeltaMessage
  | AckMessage
  | ReplicasMessage
  | CountersMessage
  | ErrorMessage
  | ServerPing;

// REST
export interface CommandResult {
  commandId?: CommandId;
  result: 'applied' | 'duplicate' | 'noop';
  revision: Revision;
  atIndex?: number;
}
export interface ApiError {
  code: 400 | 403 | 404 | 409;
  message: string;
  conflicts?: Conflict[];
}

export const LAYOUT_LABELS: Record<Layout, readonly string[]> = {
  mono: ['M'],
  stereo: ['L', 'R'],
  '5.1': ['L', 'R', 'C', 'LFE', 'Ls', 'Rs'],
  '7.1': ['L', 'R', 'C', 'LFE', 'Ls', 'Rs', 'Lrs', 'Rrs'],
};
