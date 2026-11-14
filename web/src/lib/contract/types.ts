// TypeScript view of api/ws/*.schema.json. The contract tests validate
// mock traffic typed with these interfaces against the schemas.

export type Id = string;
export type ClientId = string;
export type CommandId = string;
export type Revision = number;
export type Layout = 'mono' | 'stereo' | '5.1' | '7.1';
export type Width = 1 | 2 | 6 | 8;
export type Solo = 'off' | 'pfl' | 'afl';
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
export interface Shape {
  sampleRate: 48000;
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
}
export interface Send {
  on: boolean;
  level: number;
  pan: number;
  pre: boolean;
}
export interface ChannelState {
  trim: number;
  polarity: boolean;
  hpf: Hpf;
  eq: Eq;
  gate: Gate;
  comp: Comp;
  delay: number;
  pan: PanState;
  fader: number;
  mute: boolean;
  solo: Solo;
  sends: Record<Id, Send>;
  routes: Record<Id, boolean>;
}
export interface BusState {
  polarity: boolean;
  eq: Eq;
  gate: Gate;
  comp: Comp;
  delay: number;
  fader: number;
  mute: boolean;
}
export interface MixMinusState {
  exclude: Id[];
  fader: number;
  mute: boolean;
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
}
export interface AfvBinding {
  enabled: boolean;
  source: string | null;
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
  protocol: 'mcu' | 'hui' | 'osc';
  transport: 'alsa-midi' | 'bridge' | 'osc-udp';
  connected: boolean;
  bankOffset: number;
  strips: number;
}
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
}
export type Owners = Record<string, ClientId>;

export interface ReplicaRef {
  id: Id;
  role: ReplicaRole;
}
export interface ReplicaStatus extends ReplicaRef {
  healthy: boolean;
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
  owner: ClientId | null;
}
export interface Conflict {
  path: string;
  current: unknown;
  owner: ClientId;
}

// WebSocket client messages
export interface HelloMessage {
  type: 'hello';
  clientId: ClientId;
  lastRevision?: Revision;
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
  lastRevision?: Revision;
}
export type CommandMessage = SetMessage | TouchMessage | ReleaseMessage;
export type ClientMessage = HelloMessage | CommandMessage | ResyncMessage;

// WebSocket server messages
export interface SnapshotMessage {
  type: 'snapshot';
  clientId: ClientId;
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
  origin?: { clientId: ClientId; commandId: CommandId; seq?: number };
}
export interface ResumeMessage {
  type: 'resume';
  clientId: ClientId;
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
export interface ChannelMeter {
  peak: number[];
  rms: number[];
}
export interface Loudness {
  momentary: number | null;
  shortTerm: number | null;
  integrated: number | null;
  lra: number | null;
  integratedValid: boolean;
}
export interface BusMeter {
  peak: number[];
  truePeak: number[];
  rms: number[];
  loudness: Loudness;
}
export interface MetersMessage {
  type: 'meters';
  replica: ReplicaRef;
  channels: Record<Id, ChannelMeter>;
  buses: Record<Id, BusMeter>;
  auxes: Record<Id, BusMeter>;
  mixMinus: Record<Id, BusMeter>;
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
  owner?: ClientId;
}
export type ServerMessage =
  | SnapshotMessage
  | ResumeMessage
  | DeltaMessage
  | AckMessage
  | ReplicasMessage
  | MetersMessage
  | CountersMessage
  | ErrorMessage;

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
