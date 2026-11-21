// TypeScript view of api/ws/*.schema.json. The contract tests validate
// mock traffic typed with these interfaces against the schemas.

export type Id = string;
export type SessionId = string;
/** Published in place of a session id: base64url of the first 16 bytes of SHA-256 over it. */
export type OwnerHandle = string;
export type SessionKind = 'ui' | 'console' | 'script';
export type LinkMode = 'primary' | 'standby';
export type CommandId = string;
export type Revision = number;
/** History of the replicated state: a uint64 in decimal digits. Revisions of two epochs are not comparable. */
export type Epoch = string;
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
export interface ChannelShape extends ElementShape {
  /** A second (B) input source is configured. */
  altInput: boolean;
  /** Label of the input source, shown under the channel name. */
  source?: string;
  /** Longest delay, in samples, the input's delay line takes; absent without one, and then only 0. */
  maxDelay?: number;
  /** Present when the input is a line-up tone generator, no flow: the frequency and level it starts at. */
  tone?: ToneInputShape;
}

/** A tone channel's generator as configured. */
export interface ToneInputShape {
  /** Hz. */
  frequency: number;
  /** dBFS sample peak. */
  level: number;
}

/** A tone channel's generator: a sine on every leg of its input. */
export interface ToneInput {
  /** Whole Hz, within TONE_FREQUENCY_HZ. */
  frequency: number;
  /** dBFS sample peak, within TONE_INPUT_LEVEL_DB. */
  level: number;
  /** Stops the tone on one leg at a time to identify the legs. */
  ident: boolean;
}

/** The ranges of model ToneFrequency and ToneInputLevel: samplez's generator's. */
export const TONE_FREQUENCY_HZ = { min: 20, max: 20000 } as const;
export const TONE_INPUT_LEVEL_DB = { min: -60, max: 0 } as const;
/** Operator-assigned scribble strip colour. */
export type ScribbleColor = 'none' | 'red' | 'orange' | 'yellow' | 'green' | 'cyan' | 'blue' | 'purple';
export const SCRIBBLE_COLORS: readonly ScribbleColor[] = ['none', 'red', 'orange', 'yellow', 'green', 'cyan', 'blue', 'purple'];
export type InputSelect = 'a' | 'b';
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
export interface DuckShape {
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
  /** At most 8; absent means none. */
  ducks?: DuckShape[];
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
  /** Deprecated: stored, never heard. */
  fr: number;
  /** Deprecated: stored, never heard. */
  divergence: number;
  /** Deprecated: stored, never heard. */
  lfe: number;
  /** Front -1 to rear 1, of a mono channel on a surround bus. */
  frontRear?: number;
  /** 0 discrete centre .. 1 phantom centre. */
  spread?: number;
  /** LFE send, dB; -90 off. */
  lfeSend?: number;
}
export type StereoImage = 'normal' | 'leftToBoth' | 'rightToBoth' | 'swap';
export interface Send {
  on: boolean;
  level: number;
  pan: number;
  pre: boolean;
}
export interface ChannelState {
  input: InputSelect;
  color: ScribbleColor;
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
  /** A tone channel's generator; absent on a channel reading a flow. */
  tone?: ToneInput;
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
  eq: Eq;
  gate: Gate;
  comp: Comp;
  dynamicsBeforeEq: boolean;
  fader: number;
  mute: boolean;
  interrupt: Interrupt;
}
export interface VcaState {
  offset: number;
  mute: boolean;
  members: Id[];
  /** Listens to every member. */
  solo: Solo;
}
export interface MonitorState {
  source: Id;
  level: number;
  dim: boolean;
  cut: boolean;
  mono: boolean;
  talkbackDim: boolean;
  /** dB. */
  dimDepth: number;
  /** dB. */
  monoCompensation: number;
  downmix: 'loRo' | 'lr';
  image: StereoImage;
  /** Per leg of the monitor output. */
  speakerMute: boolean[];
  speakerSolo: boolean[];
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
  /** The session that registered it last; null until one does. */
  handle: OwnerHandle | null;
  protocol: 'mcu' | 'hui' | 'osc';
  transport: 'alsa-midi' | 'bridge' | 'osc-udp';
  connected: boolean;
  bankOffset: number;
  strips: number;
}
export type LoudnessPreset = 'ebu' | 'atsc' | 'custom';
export type LoudnessScale = 'ebu9' | 'ebu18';
/** Integration of an output's loudness measurement: start and pause (EBU Tech 3341). */
export interface LoudnessMeasurement {
  running: boolean;
  /** Resets so far; the server writes it. */
  resets?: number;
}
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
export interface Tone {
  on: boolean;
  /** dBFS. */
  level: number;
  ident: boolean;
  /** Bus, aux and mix-minus ids that carry the tone. */
  targets: Id[];
}
export type DuckMode = 'auto' | 'on' | 'off';
export type DuckKeyTap = 'preFader' | 'preFaderOpen' | 'postFader';
export interface DuckChannel {
  trigger: boolean;
  target: boolean;
  /** dB, 0..40. */
  depth: number;
}
export interface DuckState {
  mode: DuckMode;
  /** Momentary: reset by the server when its owner releases it or the lease ends. */
  hold: boolean;
  keyTap: DuckKeyTap;
  /** dBFS, -60..0. */
  threshold: number;
  /** dB, 0..12. */
  hysteresis: number;
  /** freq Hz, 20..500. */
  keyHpf: { enabled: boolean; freq: number };
  /** freq Hz, 1000..20000. */
  keyLpf: { enabled: boolean; freq: number };
  /** 1..500. */
  attackMs: number;
  /** 0..5000. */
  holdMs: number;
  /** 10..10000. */
  releaseMs: number;
  /** Keyed by channel id. */
  channels: Record<Id, DuckChannel>;
  /** Keyed by bus, aux and mix-minus id. */
  destinations: Record<Id, boolean>;
}
export interface State {
  channels: Record<Id, ChannelState>;
  buses: Record<Id, BusState>;
  auxes: Record<Id, BusState>;
  mixMinus: Record<Id, MixMinusState>;
  vcas: Record<Id, VcaState>;
  ducks: Record<Id, DuckState>;
  monitor: MonitorState;
  afv: Record<Id, AfvBinding>;
  tally: Record<string, TallyState>;
  recallSafe: Record<Id, RecallGroup[]>;
  scenes: Scene[];
  consoles: ConsoleStatus[];
  /** Loudness measurement per bus, aux and mix-minus output; not part of scenes. */
  loudness: Record<Id, LoudnessMeasurement>;
  metering: Metering;
  tone: Tone;
  /** Last recalled scene; server-written. */
  sceneRecalled: { id: Id; at: string } | null;
}
export interface Owner {
  principal: string;
  handle: OwnerHandle;
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
/** What a replica was built from: the faderz version and the samplez release it was built against. */
export interface Build {
  faderz: string;
  samplez: string;
}
/** GET /about: what the serving replica was built from, and the faderz changelog of that release. */
export interface About {
  build: Build;
  /** The faderz CHANGELOG.md, Markdown as release-please writes it. */
  changelog: string;
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
  /** True peak at the meter tap. */
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
/** What the monitor plays before level, dim and cut: the listen sum (PFL over AFL) or the source. */
export interface MonitorMeter {
  peak: number[];
}
/** A duck group's readings: its targets are lowered by their depth times envelope, dB. */
export interface DuckMeter {
  /** Largest level of the group's keys after their key filters and key gains, dBFS; null while the group is not in use. */
  key: number | null;
  /** 0 (not at all) to 1 (by the full depth). */
  envelope: number;
  /** The detector is triggered or holding. */
  engaged: boolean;
}

// WebSocket client messages
export interface HelloMessage {
  type: 'hello';
  session: SessionId;
  kind: 'ui' | 'console';
  mode: LinkMode;
  /** Sent together with epoch. */
  lastRevision?: Revision;
  /** Epoch of lastRevision. */
  epoch?: Epoch;
}
export interface PromoteMessage {
  type: 'promote';
  /** Highest revision the client has applied; commands wait until the replica has applied it. */
  lastRevision: Revision;
  /** Epoch of lastRevision: the replica waits for it only in its own epoch. */
  epoch: Epoch;
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
  /** On the counter of the control that set, touch and release share. */
  seq: number;
  path: string;
}
export interface ReleaseMessage {
  type: 'release';
  id: CommandId;
  /** On the counter of the control that set, touch and release share. */
  seq: number;
  path: string;
}
export interface ResyncMessage {
  type: 'resync';
  /** Highest revision the client has applied; the answer waits until the replica has applied it. */
  lastRevision?: Revision;
  /** Epoch of lastRevision; sent together with it. */
  epoch?: Epoch;
  /** Answer with a snapshot. */
  full?: true;
}
/** Asks the session that owns a control to give it up; not a command, not answered. */
export interface RequestMessage {
  type: 'request';
  path: string;
}
/** The element this session has selected for editing, or null; not a command, not answered. */
export interface SelectMessage {
  type: 'select';
  element: Id | null;
}
/** Registers a console as this session's, or updates it; not a command, answered only when refused. */
export interface ConsoleMessage {
  type: 'console';
  id: Id;
  name: string;
  protocol: ConsoleStatus['protocol'];
  transport: ConsoleStatus['transport'];
  strips: number;
  bankOffset: number;
}
export type CommandMessage = SetMessage | TouchMessage | ReleaseMessage;
export type ClientMessage =
  | HelloMessage
  | PromoteMessage
  | CommandMessage
  | RequestMessage
  | SelectMessage
  | ConsoleMessage
  | ResyncMessage
  | CreditMessage
  | PingMessage
  | PongMessage;

// WebSocket server messages (JSON text); meter frames are binary
export interface SnapshotMessage {
  type: 'snapshot';
  you: Owner;
  replica: ReplicaRef;
  epoch: Epoch;
  revision: Revision;
  shape: Shape;
  state: State;
  owners: Owners;
  /** The replica could not confirm this is the cluster's latest state; revision may be older than asked for. */
  stale?: true;
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
export interface AheadMessage {
  type: 'ahead';
  replica: ReplicaRef;
  /** The cluster epoch, the one the hello or resync carried. */
  epoch: Epoch;
  /** Highest revision the cluster has committed. */
  revision: Revision;
  lastRevision: Revision;
}
export interface ResumeMessage {
  type: 'resume';
  you: Owner;
  replica: ReplicaRef;
  epoch: Epoch;
  revision: Revision;
  deltas: DeltaMessage[];
  /** As SnapshotMessage.stale. */
  stale?: true;
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
  build: Build;
  replicas: ReplicaStatus[];
}
/** A channel's input as the engine read it, beside the counters (model.schema.json InputHealth). */
export interface InputHealth {
  channel: Id;
  state: 'attached' | 'pruned' | 'detached';
  up: boolean;
  lateBlocks: number;
  tooLateBlocks: number;
}
/** A bus, aux or mix-minus output's loudness measurement as the replica's engine runs it (model.schema.json LoudnessHealth). */
export interface LoudnessHealth {
  id: Id;
  /** 10 ms slices that were gaps: an input down, or a loudness not finite. Equal on every replica that lost none. */
  gaps: number;
  /** 10 ms slices this replica did not meter whole. */
  lost: number;
  /** When the measurement started, RFC 3339 UTC; null before it measured. */
  since: string | null;
  /** It did not start at the last reset this replica applied, or none was applied. */
  restarted: boolean;
}
export interface CountersMessage {
  type: 'counters';
  replica: ReplicaRef;
  values: Record<string, number>;
  inputs: InputHealth[];
  /** Absent from a replica older than it, as during a rolling upgrade. */
  loudness?: LoudnessHealth[];
}
export interface ErrorMessage {
  type: 'error';
  replica: ReplicaRef;
  /** 503: not applied; the client sends the command again with its id. */
  code: 400 | 404 | 409 | 429 | 503;
  message: string;
  commandId?: CommandId;
  path?: string;
  current?: unknown;
  owner?: Owner;
}
export interface Selection {
  session: Owner;
  element: Id;
}
export interface SelectionsMessage {
  type: 'selections';
  replica: ReplicaRef;
  selections: Selection[];
}
/** A session connected to the replica that sent the presence message. */
export interface PresentSession {
  handle: OwnerHandle;
  kind: 'ui' | 'console';
  /** Display name of who it is signed in as, or token or anonymous. */
  user: string;
  role: 'operator' | 'viewer';
  /** RFC 3339: when its first connection to this replica opened. */
  since: string;
  /** RFC 3339, to the second: its last command, selection or control request. */
  active: string;
}
export interface PresenceMessage {
  type: 'presence';
  replica: ReplicaRef;
  sessions: PresentSession[];
}
export interface ControlRequestMessage {
  type: 'controlRequest';
  replica: ReplicaRef;
  path: string;
  from: Owner;
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
  | AheadMessage
  | SelectionsMessage
  | PresenceMessage
  | ControlRequestMessage
  | ServerPing;

// REST
export interface CommandResult {
  commandId?: CommandId;
  result: 'applied' | 'duplicate' | 'noop';
  revision: Revision;
  atIndex?: number;
}
export interface RecallResult extends CommandResult {
  /** Channels whose stored delay is past their delay line: the recall set their delay to 0. */
  delaysNotApplied?: Id[];
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
