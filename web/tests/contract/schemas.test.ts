import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { Ajv2020 } from 'ajv/dist/2020.js';
import addFormatsModule from 'ajv-formats';
import { parse as parseYaml } from 'yaml';
import { API_DIR, Contract } from '../../dev/mock/contract';
import { CONTINUOUS_PATH_PATTERNS, isContinuousPath } from '../../src/lib/contract/continuous';

const addFormats = addFormatsModule as unknown as (ajv: Ajv2020) => Ajv2020;
const FILES = ['model.schema.json', 'counters.schema.json', 'client-message.schema.json', 'server-message.schema.json'];
const read = (f: string) => JSON.parse(readFileSync(join(API_DIR, 'ws', f), 'utf8')) as Record<string, unknown>;

describe('JSON Schemas', () => {
  it('compile under ajv strict mode', () => {
    // strictRequired would refuse the Delta anyOf of required-only branches,
    // a standard 2020-12 idiom for "at least one of".
    const ajv = new Ajv2020({ strict: true, strictRequired: false, allErrors: true });
    addFormats(ajv);
    for (const f of FILES) ajv.addSchema(read(f));
    for (const f of FILES) expect(() => ajv.getSchema(read(f).$id as string)).not.toThrow();
  });

  it('contain ASCII only', () => {
    for (const f of [...FILES.map((f) => `ws/${f}`), 'openapi.yaml']) {
      expect(readFileSync(join(API_DIR, f), 'utf8')).toMatch(/^[\x09\x0a\x0d\x20-\x7e]*$/);
    }
  });
});

describe('openapi.yaml', () => {
  const doc = parseYaml(readFileSync(join(API_DIR, 'openapi.yaml'), 'utf8')) as Record<string, unknown>;

  function refs(node: unknown, out: string[] = []): string[] {
    if (Array.isArray(node)) node.forEach((n) => refs(n, out));
    else if (node && typeof node === 'object') {
      for (const [k, v] of Object.entries(node)) {
        if (k === '$ref' && typeof v === 'string') out.push(v);
        else refs(v, out);
      }
    }
    return out;
  }

  function resolve(root: unknown, pointer: string): unknown {
    return pointer
      .split('/')
      .slice(1)
      .map((t) => t.replace(/~1/g, '/').replace(/~0/g, '~'))
      .reduce<unknown>((n, t) => (n as Record<string, unknown> | undefined)?.[t], root);
  }

  it('is OpenAPI 3.1 with the JSON Schema 2020-12 dialect', () => {
    expect(doc['openapi']).toBe('3.1.0');
    expect(doc['jsonSchemaDialect']).toBe('https://json-schema.org/draft/2020-12/schema');
  });

  it('resolves every $ref', () => {
    const all = refs(doc);
    expect(all.length).toBeGreaterThan(20);
    for (const ref of all) {
      const [file, fragment = ''] = ref.split('#');
      const target = file ? read(file.replace('./ws/', '')) : doc;
      expect(resolve(target, fragment), ref).toBeDefined();
    }
  });

  it('compiles every request and response schema', () => {
    const contract = new Contract();
    const paths = doc['paths'] as Record<string, Record<string, { requestBody?: unknown }>>;
    for (const [path, ops] of Object.entries(paths)) {
      for (const [method, op] of Object.entries(ops)) {
        if (method === 'parameters') continue;
        if (op.requestBody) expect(contract.checkRequest(path, method, {})).not.toBeNull();
      }
    }
  });
});

describe('Change values are constrained by path', () => {
  const contract = new Contract();
  const set = (path: string, value: unknown) =>
    contract.checkClientMessage({ type: 'set', id: crypto.randomUUID(), seq: 1, path, value });

  it.each([
    ['/channels/ch1/fader', 0],
    ['/channels/ch1/fader', -90],
    ['/channels/ch1/eq/bands/3/type', 'highShelf'],
    ['/channels/ch1/sends/aux1/pre', true],
    ['/channels/ch1/routes/pgm51', false],
    ['/channels/ch1/solo', 'afl'],
    ['/vcas/vca1/members', ['ch1', 'ch2']],
    ['/monitor/source', 'pgm20'],
    ['/afv/ch13/source', null],
    ['/recallSafe/ch1', ['fader', 'mute']],
    ['/channels/st1/polarity/1', true],
    ['/channels/sur1/legTrim/5', -3.5],
    ['/buses/pgm51/polarity/3', true],
    ['/channels/ch1/meterTap', 'input'],
    ['/channels/ch1/directOut/tap', 'preFader'],
    ['/metering/zones/alignment', -20],
    ['/metering/loudness/preset', 'atsc'],
    ['/metering/loudness/target', -24],
    ['/metering/loudness/tolerance', 0.5],
    ['/metering/scale', 'ebu18'],
    ['/channels/ch1/pan/spread', 0.4],
    ['/channels/ch1/pan/frontRear', -0.5],
    ['/channels/ch1/pan/lfeSend', -10],
    ['/channels/st1/image', 'leftToBoth'],
    ['/channels/ch1/delay', 240000],
    ['/channels/ch1/dynamicsBeforeEq', true],
    ['/buses/pgm20/limiter/ceiling', -1.5],
    ['/auxes/aux1/limiter/enabled', true],
    ['/afv/ch13/riseMs', 0],
    ['/afv/ch13/fallMs', 400],
    ['/afv/ch13/holdMs', 250],
    ['/afv/ch13/offLevel', -20],
    ['/mixMinus/mm1/interrupt/source', 'ch1'],
    ['/mixMinus/mm1/interrupt/source', null],
    ['/mixMinus/mm1/interrupt/active', true],
    ['/monitor/talkbackDim', false],
    ['/tone/on', true],
    ['/tone/level', -20],
    ['/tone/ident', true],
    ['/tone/targets', ['pgm20', 'aux1']],
    ['/monitor/dimDepth', 20],
    ['/monitor/monoCompensation', 3],
    ['/monitor/downmix', 'lr'],
    ['/monitor/image', 'swap'],
    ['/monitor/speakerMute/1', true],
    ['/monitor/speakerSolo/0', false],
    ['/vcas/vca1/solo', 'pfl'],
    ['/channels/ch1/input', 'b'],
    ['/loudness/pgm20/running', false],
    ['/loudness/mm1/running', true],
  ])('accepts %s = %j', (path, value) => {
    expect(set(path, value)).toBeNull();
  });

  it.each([
    ['/channels/ch1/fader', 10.5],
    ['/channels/ch1/fader', 'loud'],
    ['/channels/ch1/eq/bands/4/gain', 0],
    ['/channels/ch1/eq/bands/0/gain', 19],
    ['/channels/ch1/solo', 'solo'],
    ['/channels/ch1/mute', 1],
    ['/buses/pgm51/trim', 0],
    ['/buses/pgm51/pan/lr', 0],
    ['/afv/ch13/open', true],
    ['/sceneRecalled', null],
    ['/vcas/vca1/members', ['ch1', 'ch1']],
    ['/recallSafe/ch1', ['everything']],
    ['/monitor/level', 11],
    ['/channels/ch1/polarity', true],
    ['/channels/ch1/polarity/8', true],
    ['/channels/ch1/polarity/0', 1],
    ['/channels/ch1/legTrim/0', 20.5],
    ['/buses/pgm51/legTrim/0', 0],
    ['/channels/ch1/meterTap', 'post'],
    ['/channels/ch1/directOut', { tap: 'input' }],
    ['/buses/pgm51/meterTap', 'input'],
    ['/metering/zones/alignment', 3],
    ['/metering/loudness/preset', 'r128'],
    ['/metering/loudness/target', -13],
    ['/metering/loudness/tolerance', 0],
    ['/metering/scale', 'ebu27'],
    ['/metering/zones', { alignment: -18, permittedMax: -9, truePeakMax: -1 }],
    ['/channels/ch1/pan/spread', 1.5],
    ['/channels/ch1/pan/frontRear', 1.5],
    ['/channels/ch1/pan/lfeSend', 12],
    ['/channels/st1/image', 'mono'],
    ['/buses/pgm20/image', 'swap'],
    ['/channels/ch1/delay', 240001],
    ['/channels/ch1/delay', 1.5],
    ['/channels/ch1/limiter/enabled', true],
    ['/buses/pgm20/limiter/ceiling', 0.5],
    ['/buses/pgm20/dynamicsBeforeEq', 'yes'],
    ['/afv/ch13/riseMs', 12.5],
    ['/afv/ch13/holdMs', 10001],
    ['/afv/ch13/offLevel', 3],
    ['/mixMinus/mm1/interrupt/active', 'on'],
    ['/mixMinus/mm1/interrupt', { source: 'ch1', active: true }],
    ['/auxes/aux1/interrupt/active', true],
    ['/tone/level', -31],
    ['/tone/level', 1],
    ['/tone/frequency', 440],
    ['/tone/targets', ['pgm20', 'pgm20']],
    ['/monitor/dimDepth', 61],
    ['/monitor/monoCompensation', -1],
    ['/monitor/downmix', 'ltRt'],
    ['/monitor/speakerMute/8', true],
    ['/monitor/speakerMute', [true, false]],
    ['/vcas/vca1/solo', true],
    ['/channels/ch1/input', 'c'],
    ['/buses/pgm20/input', 'a'],
    ['/loudness/pgm20/running', 'paused'],
    ['/loudness/pgm20', { running: false }],
    ['/loudness/pgm20/integrated', null],
  ])('refuses %s = %j', (path, value) => {
    expect(set(path, value)).not.toBeNull();
  });
});

describe('Duck group controls', () => {
  const contract = new Contract();
  const set = (path: string, value: unknown) =>
    contract.checkClientMessage({ type: 'set', id: crypto.randomUUID(), seq: 1, path, value });

  it.each([
    ['/ducks/comm/mode', 'on'],
    ['/ducks/comm/hold', true],
    ['/ducks/comm/keyTap', 'preFaderOpen'],
    ['/ducks/comm/threshold', -60],
    ['/ducks/comm/hysteresis', 12],
    ['/ducks/comm/keyHpf/freq', 20],
    ['/ducks/comm/keyLpf/enabled', true],
    ['/ducks/comm/keyLpf/freq', 1000],
    ['/ducks/comm/attackMs', 500],
    ['/ducks/comm/holdMs', 0],
    ['/ducks/comm/releaseMs', 10000],
    ['/ducks/comm/channels/ch1/trigger', true],
    ['/ducks/comm/channels/ch1/target', false],
    ['/ducks/comm/channels/ch1/depth', 40],
    ['/ducks/comm/destinations/mm1', true],
  ])('takes %s = %j', (path, value) => {
    expect(set(path, value)).toBeNull();
  });

  it.each([
    ['/ducks/comm/mode', 'bypass'],
    ['/ducks/comm/hold', 1],
    ['/ducks/comm/keyTap', 'input'],
    ['/ducks/comm/threshold', -61],
    ['/ducks/comm/hysteresis', 12.5],
    ['/ducks/comm/keyHpf/freq', 600],
    ['/ducks/comm/keyLpf/freq', 900],
    ['/ducks/comm/attackMs', 0.5],
    ['/ducks/comm/holdMs', 5001],
    ['/ducks/comm/releaseMs', 5],
    ['/ducks/comm/channels/ch1/depth', 41],
    ['/ducks/comm/destinations/mm1', 'yes'],
    ['/ducks/comm/channels/ch1', { trigger: true, target: false, depth: 10 }],
  ])('refuses %s = %j', (path, value) => {
    expect(set(path, value)).not.toBeNull();
  });
});

describe('ContinuousPath', () => {
  const model = read('model.schema.json') as { $defs: Record<string, { anyOf: Array<{ pattern: string }> }> };
  const ajv = new Ajv2020({ strict: true });
  for (const f of FILES) ajv.addSchema(read(f));
  const controlPath = ajv.compile({ $ref: 'https://github.com/danielclasen/faderz/raw/main/api/ws/model.schema.json#/$defs/ControlPath' });

  it('is declared in the contract with the patterns the client coalesces', () => {
    expect(model.$defs['ContinuousPath']!.anyOf.map((p) => p.pattern)).toEqual(CONTINUOUS_PATH_PATTERNS);
  });

  it.each([
    '/channels/ch1/fader',
    '/channels/sur1/legTrim/3',
    '/channels/ch1/sends/aux1/level',
    '/channels/ch1/pan/lfeSend',
    '/channels/ch1/eq/bands/2/q',
    '/buses/pgm51/comp/makeup',
    '/auxes/aux1/delay',
    '/mixMinus/mm1/fader',
    '/vcas/vca1/offset',
    '/monitor/level',
    '/ducks/comm/threshold',
    '/ducks/comm/keyLpf/freq',
    '/ducks/comm/releaseMs',
    '/ducks/comm/channels/ch1/depth',
  ])('%s is a continuous control path', (path) => {
    expect(controlPath(path)).toBe(true);
    expect(isContinuousPath(path)).toBe(true);
  });

  it.each([
    '/channels/ch1/mute',
    '/channels/ch1/polarity/0',
    '/channels/ch1/eq/bands/0/type',
    '/channels/st1/image',
    '/channels/ch1/sends/aux1/pre',
    '/monitor/source',
    '/vcas/vca1/members',
    '/afv/ch13/source',
    '/ducks/comm/mode',
    '/ducks/comm/hold',
    '/ducks/comm/keyTap',
    '/ducks/comm/keyHpf/enabled',
    '/ducks/comm/channels/ch1/trigger',
    '/ducks/comm/destinations/pgm20',
  ])('%s is a discrete control path', (path) => {
    expect(controlPath(path)).toBe(true);
    expect(isContinuousPath(path)).toBe(false);
  });
});

describe('WebSocket messages', () => {
  const contract = new Contract();
  const server = (msg: unknown) => {
    try {
      contract.assertServerMessage(msg);
      return null;
    } catch (e) {
      return (e as Error).message;
    }
  };
  const replica = { id: 'r1', role: 'leader' };
  const owner = { principal: 'anonymous', handle: 'q3Jx0fZL1kWm7d2Y-0b_Aw', kind: 'ui', connection: 'c1' };
  const present = { handle: owner.handle, kind: 'ui', user: 'alice', role: 'operator', since: '2026-11-17T09:00:00.000Z', active: '2026-11-17T09:05:12.000Z' };
  const cmd = '0b6e3a4c-5d1f-4c2a-9e8b-7f6a5d4c3b2a';
  const cmd2 = '1c7f4b5d-6e2a-4d3b-8f9c-8a7b6e5d4c3b';
  /** The largest uint64. */
  const epoch = '18446744073709551615';

  it.each([
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'Y29uc29sZS1tY3UtMS1zZXNz', kind: 'console', mode: 'standby', lastRevision: 12, epoch },
    { type: 'hello', session: 'Y29uc29sZS1tY3UtMS1zZXNz', kind: 'ui', mode: 'primary', lastRevision: 0, epoch: '0' },
    { type: 'promote', lastRevision: 4, epoch },
    { type: 'resync', lastRevision: 4, epoch, full: true },
    { type: 'credit', n: 1 },
    { type: 'credit', n: 8 },
    { type: 'credit', n: 100 },
    { type: 'ping', t: 0 },
    { type: 'pong', t: 1731590000000 },
    { type: 'resync' },
    { type: 'touch', id: cmd, seq: 2, path: '/channels/ch1/fader' },
    { type: 'release', id: cmd, seq: 3, path: '/channels/ch1/fader' },
    { type: 'request', path: '/mixMinus/mm1/interrupt/active' },
    { type: 'select', element: 'ch3' },
    { type: 'select', element: null },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'osc-1', name: 'Tablet', protocol: 'osc', transport: 'osc-udp', strips: 16, bankOffset: 24 },
  ])('client accepts %j', (msg) => {
    expect(contract.checkClientMessage(msg)).toBeNull();
  });

  it.each([
    { type: 'hello', clientId: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui' },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', mode: 'primary' },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'script', mode: 'primary' },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', mode: 'backup' },
    { type: 'hello', session: 'dGFiIDEt c2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'tab-1', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA==', kind: 'ui', mode: 'primary' },
    { type: 'promote', mode: 'primary' },
    { type: 'promote' },
    { type: 'resync', lastRevision: 4, epoch, full: false },
    // A revision means nothing without its epoch, and the reverse.
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary', lastRevision: 12 },
    { type: 'hello', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', mode: 'primary', epoch },
    { type: 'promote', lastRevision: 4 },
    { type: 'resync', lastRevision: 4 },
    { type: 'resync', epoch },
    // Text, so that no uint64 loses digits in a JSON number.
    { type: 'resync', lastRevision: 4, epoch: 7 },
    { type: 'resync', lastRevision: 4, epoch: '07' },
    { type: 'resync', lastRevision: 4, epoch: '-1' },
    { type: 'resync', lastRevision: 4, epoch: '123456789012345678901' },
    { type: 'credit' },
    { type: 'credit', n: 0 },
    { type: 'credit', n: -1 },
    { type: 'credit', n: 1.5 },
    { type: 'credit', n: 1, channels: ['ch1'] },
    // Touch and release are ordered on the control's seq counter, like set.
    { type: 'touch', id: cmd, path: '/channels/ch1/fader' },
    { type: 'release', id: cmd, path: '/channels/ch1/fader' },
    { type: 'release', id: cmd, seq: 0, path: '/channels/ch1/fader' },
    { type: 'ping' },
    { type: 'ping', t: -1 },
    { type: 'ping', t: 1, revision: 4 },
    { type: 'pong', t: 1, replica },
    { type: 'meters' },
    // A request and a selection are not commands: no id, no seq.
    { type: 'request', id: cmd, path: '/channels/ch1/fader' },
    { type: 'request', path: '/channels/ch1/peak' },
    { type: 'request' },
    { type: 'select' },
    { type: 'select', element: 'Ch 3' },
    { type: 'select', element: ['ch1', 'ch2'] },
    { type: 'select', id: cmd, seq: 1, element: 'ch1' },
    // A registration is not a command either, and carries every field.
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', bankOffset: 0 },
    { type: 'console', id: 'xt-1', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'XT 1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: '', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: 'x'.repeat(65), protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'midi', transport: 'bridge', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'usb', strips: 8, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 0, bankOffset: 0 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: -1 },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0, connected: true },
    { type: 'console', id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0, handle: owner.handle },
    { type: 'console', seq: 1, id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', strips: 8, bankOffset: 0 },
  ])('client refuses %j', (msg) => {
    expect(contract.checkClientMessage(msg)).not.toBeNull();
  });

  const delta = {
    type: 'delta',
    replica,
    revision: 7,
    atIndex: 480000,
    changes: [
      { path: '/channels/ch1/fader', value: -5 },
      { path: '/channels/ch2/mute', value: true },
    ],
    owners: [{ path: '/channels/ch1/fader', owner }],
    origins: [
      { owner, commandId: cmd, seq: 1731590000001 },
      { owner: { ...owner, handle: 'Rk9vYmFyLWJhei1xdXV4LT', kind: 'console', connection: 'c2' }, commandId: cmd2 },
    ],
  };

  it.each([
    ['delta with several origins', delta],
    ['delta releasing ownership', { type: 'delta', replica, revision: 8, owners: [{ path: '/channels/ch1/fader', owner: null }] }],
    ['resume', { type: 'resume', you: owner, replica, epoch, revision: 7, deltas: [delta] }],
    ['stale resume', { type: 'resume', you: owner, replica, epoch, revision: 7, deltas: [], stale: true }],
    ['script owner', { type: 'delta', replica, revision: 9, owners: [{ path: '/buses/pgm/fader', owner: { ...owner, kind: 'script' } }] }],
    ['replicas', { type: 'replicas', replica, build: { faderz: '0.4.3', samplez: '0.12.3' }, replicas: [{ ...replica, healthy: true, commitable: true }, { id: 'r2', role: 'follower', healthy: true, commitable: false }] }],
    ['ping', { type: 'ping', replica, t: 0 }],
    ['pong', { type: 'pong', replica, t: 5 }],
    ['409 with owner', { type: 'error', replica, code: 409, message: 'owned', commandId: cmd, path: '/channels/ch1/fader', current: -3, owner }],
    // <replica>:<32 hex>:<n> with a 63-character replica id.
    ['owner of a long connection id', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, connection: `${'r'.repeat(63)}:${'0f'.repeat(16)}:18446744073709551615` } }] }],
    ['503 not applied', { type: 'error', replica, code: 503, message: 'not applied', commandId: cmd, path: '/channels/ch1/fader' }],
    ['counters', { type: 'counters', replica, values: { deadline_miss_total: 0 }, inputs: [{ channel: 'ch1', state: 'pruned', up: false, lateBlocks: 3, tooLateBlocks: 0 }], loudness: [{ id: 'pgm', gaps: 2, lost: 1, since: '2026-11-20T09:14:03.120Z', restarted: true }] }],
    ['ahead', { type: 'ahead', replica, epoch, revision: 120, lastRevision: 245 }],
    ['selections', { type: 'selections', replica, selections: [{ session: owner, element: 'ch3' }, { session: { ...owner, handle: 'Rk9vYmFyLWJhei1xdXV4LT' }, element: 'pgm20' }] }],
    ['no selections', { type: 'selections', replica, selections: [] }],
    ['presence', { type: 'presence', replica, sessions: [present, { ...present, handle: 'Rk9vYmFyLWJhei1xdXV4LT', kind: 'console', user: 'token', role: 'viewer' }] }],
    ['no presence', { type: 'presence', replica, sessions: [] }],
    ['control request', { type: 'controlRequest', replica, path: '/mixMinus/mm1/interrupt/active', from: owner }],
    ['console refused without a command', { type: 'error', replica, code: 409, message: 'console xt-1 is connected to another session' }],
    ['console beyond the rate limit', { type: 'error', replica, code: 429, message: 'at most 20 console messages per second' }],
    [
      'consoles, one no session registered',
      {
        type: 'delta',
        replica,
        revision: 10,
        atIndex: 480000,
        changes: [
          {
            path: '/consoles',
            value: [
              { id: 'xt-1', name: 'X-Touch', handle: owner.handle, protocol: 'mcu', transport: 'bridge', connected: true, bankOffset: 8, strips: 8 },
              { id: 'xt-2', name: 'Extender', handle: null, protocol: 'mcu', transport: 'bridge', connected: false, bankOffset: 0, strips: 8 },
            ],
          },
        ],
      },
    ],
  ])('server accepts %s', (_, msg) => {
    expect(server(msg)).toBeNull();
  });

  it.each([
    ['meters', { type: 'meters', replica, channels: {}, buses: {}, auxes: {}, mixMinus: {} }],
    ['resume without you', { type: 'resume', replica, epoch, revision: 7, deltas: [] }],
    ['you as a client id', { type: 'resume', you: 'dGFiLTEtc2Vzc2lvbi1pZA', replica, epoch, revision: 7, deltas: [] }],
    ['resume without epoch', { type: 'resume', you: owner, replica, revision: 7, deltas: [] }],
    ['stale false', { type: 'resume', you: owner, replica, epoch, revision: 7, deltas: [], stale: false }],
    ['ahead without epoch', { type: 'ahead', replica, revision: 120, lastRevision: 245 }],
    ['epoch as a number', { type: 'ahead', replica, epoch: 7, revision: 120, lastRevision: 245 }],
    ['owner as a client id', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: 'dGFiLTEtc2Vzc2lvbi1pZA' }] }],
    ['owner without connection', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { principal: 'anonymous', handle: owner.handle, kind: 'ui' } }] }],
    // The session id is a credential and never leaves its client.
    ['owner with a session id', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, session: 'dGFiLTEtc2Vzc2lvbi1pZA' } }] }],
    ['owner with a session id for a handle', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { principal: 'anonymous', session: 'dGFiLTEtc2Vzc2lvbi1pZA', kind: 'ui', connection: 'c1' } }] }],
    ['handle of the wrong length', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, handle: 'dGFiLTEtc2Vzc2lvbi1pZA0' } }] }],
    ['owner of unknown kind', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, kind: 'robot' } }] }],
    ['owner with a connection id over 128 characters', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, connection: 'c'.repeat(129) } }] }],
    ['owner with empty principal', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, principal: '' } }] }],
    ['origin without owner', { ...delta, origins: [{ commandId: cmd }] }],
    ['origin with seq 0', { ...delta, origins: [{ owner, commandId: cmd, seq: 0 }] }],
    ['empty origins', { ...delta, origins: [] }],
    ['replica without commitable', { type: 'replicas', replica, build: { faderz: '0.4.3', samplez: '0.12.3' }, replicas: [{ ...replica, healthy: true }] }],
    ['ping with state', { type: 'ping', replica, t: 1, revision: 3 }],
    ['ping without replica', { type: 'ping', t: 1 }],
    ['error with a code outside the contract', { type: 'error', replica, code: 500, message: 'oops', commandId: cmd }],
    ['selection of nothing', { type: 'selections', replica, selections: [{ session: owner, element: null }] }],
    ['selection by a session id', { type: 'selections', replica, selections: [{ session: 'dGFiLTEtc2Vzc2lvbi1pZA', element: 'ch1' }] }],
    ['presence with a principal', { type: 'presence', replica, sessions: [{ ...present, principal: 'alice@example.org' }] }],
    // The session id is a credential and never leaves its client.
    ['presence with a session id', { type: 'presence', replica, sessions: [{ ...present, session: 'dGFiLTEtc2Vzc2lvbi1pZA' }] }],
    ['presence with a connection id', { type: 'presence', replica, sessions: [{ ...present, connection: 'c1' }] }],
    ['presence without a user', { type: 'presence', replica, sessions: [{ ...present, user: undefined }] }],
    ['presence with an empty user', { type: 'presence', replica, sessions: [{ ...present, user: '' }] }],
    ['presence of a script', { type: 'presence', replica, sessions: [{ ...present, kind: 'script' }] }],
    ['presence without since', { type: 'presence', replica, sessions: [{ ...present, since: undefined }] }],
    ['control request without asker', { type: 'controlRequest', replica, path: '/channels/ch1/fader' }],
    ['control request for a meter', { type: 'controlRequest', replica, path: '/channels/ch1/peak', from: owner }],
    [
      'console without a handle',
      {
        type: 'delta',
        replica,
        revision: 10,
        atIndex: 480000,
        changes: [{ path: '/consoles', value: [{ id: 'xt-1', name: 'X-Touch', protocol: 'mcu', transport: 'bridge', connected: false, bankOffset: 0, strips: 8 }] }],
      },
    ],
  ])('server refuses %s', (_, msg) => {
    expect(server(msg)).not.toBeNull();
  });

  it('carry meters only as binary frames', () => {
    const s = read('server-message.schema.json') as { oneOf: Array<{ $ref: string }>; $defs: Record<string, { examples?: unknown[] }> };
    expect(s.$defs['Meters']).toBeUndefined();
    expect(s.oneOf.map((o) => o.$ref)).not.toContain('#/$defs/MeterFrame');
    expect(JSON.stringify(s)).not.toContain('"const":"meters"');
    expect(s.$defs['MeterFrame']?.examples).toHaveLength(1);
  });

  it('owners map control paths to owner records', () => {
    const owners = contract.ajv.compile({ $ref: 'https://github.com/danielclasen/faderz/raw/main/api/ws/model.schema.json#/$defs/Owners' });
    expect(owners({ '/channels/ch1/fader': owner })).toBe(true);
    expect(owners({ '/channels/ch1/fader': 'dGFiLTEtc2Vzc2lvbi1pZA' })).toBe(false);
    expect(owners({ '/channels/ch1/peak': owner })).toBe(false);
  });
});

describe('the replicas message names the build of its replica', () => {
  const contract = new Contract();
  const replica = { id: 'r1', role: 'leader' };
  const message = (build: unknown) => ({ type: 'replicas', replica, ...(build === undefined ? {} : { build }), replicas: [{ ...replica, healthy: true, commitable: true }] });

  it('accepts a release and a samplez commit no tag names', () => {
    expect(() => contract.assertServerMessage(message({ faderz: '0.4.3', samplez: '0.12.3' }))).not.toThrow();
    expect(() => contract.assertServerMessage(message({ faderz: '0.4.3', samplez: '0.12.3+eeab3c3' }))).not.toThrow();
  });

  it('refuses one without a build, or with a part missing, empty or unknown', () => {
    for (const build of [undefined, { faderz: '0.4.3' }, { faderz: '', samplez: '0.12.3' }, { faderz: '0.4.3', samplez: '0.12.3', commit: 'eeab3c3' }]) {
      expect(() => contract.assertServerMessage(message(build)), JSON.stringify(build)).toThrow();
    }
  });

  it('holds for GET /replicas as well', () => {
    const { type: _, ...body } = message({ faderz: '0.4.3', samplez: '0.12.3' });
    expect(() => contract.assertResponse('/replicas', 'GET', 200, body)).not.toThrow();
    expect(() => contract.assertResponse('/replicas', 'GET', 200, { replica, replicas: body.replicas })).toThrow();
  });
});

describe('engine counters', () => {
  const schema = read('counters.schema.json') as { properties: Record<string, { title?: string; description?: string }> };
  const names = Object.keys(schema.properties);

  it('give every counter a name for people and an explanation, as the console shows them', () => {
    expect(names.length).toBeGreaterThan(10);
    for (const name of names) {
      const { title, description } = schema.properties[name]!;
      expect(name, name).toMatch(/^[a-z][a-z0-9_]*_total$/);
      expect(title?.length, `${name} title`).toBeGreaterThan(0);
      // One or two short sentences.
      expect(description?.split(/\. /).length, `${name} description`).toBeLessThanOrEqual(3);
      expect(description?.length, `${name} description`).toBeLessThanOrEqual(260);
    }
    expect(new Set(names.map((n) => schema.properties[n]!.title)).size, 'titles are distinct').toBe(names.length);
  });

  it('refuse a counters message with a counter they do not name, or without the inputs; take one without the loudness, as an older replica sends during a rolling upgrade', () => {
    const contract = new Contract();
    const replica = { id: 'r1', role: 'leader' };
    const inputs = [{ channel: 'ch1', state: 'attached', up: true, lateBlocks: 0, tooLateBlocks: 0 }];
    const loudness: unknown[] = [];
    expect(() => contract.assertServerMessage({ type: 'counters', replica, values: { deadline_miss_total: 1 }, inputs, loudness })).not.toThrow();
    expect(() => contract.assertServerMessage({ type: 'counters', replica, values: { afv_late_total: 1 }, inputs, loudness })).toThrow();
    expect(() => contract.assertServerMessage({ type: 'counters', replica, values: { deadline_miss_total: 1 }, loudness })).toThrow();
    expect(() => contract.assertServerMessage({ type: 'counters', replica, values: { deadline_miss_total: 1 }, inputs })).not.toThrow();
    expect(() => contract.assertServerMessage({ type: 'counters', replica, values: {}, inputs: [{ ...inputs[0], state: 'gone' }], loudness })).toThrow();
  });

  it('take a loudness measurement with its gaps, lost slices, start and restart, and refuse one without', () => {
    const contract = new Contract();
    const replica = { id: 'r1', role: 'leader' };
    const ok = { id: 'pgm', gaps: 0, lost: 0, since: null, restarted: false };
    const message = (l: object) => ({ type: 'counters', replica, values: {}, inputs: [], loudness: [l] });
    expect(() => contract.assertServerMessage(message(ok))).not.toThrow();
    expect(() => contract.assertServerMessage(message({ ...ok, since: '2026-11-20T09:14:03.120Z', restarted: true }))).not.toThrow();
    for (const bad of [{ ...ok, gaps: -1 }, { ...ok, since: 'yesterday' }, { ...ok, restarted: undefined }, { ...ok, sinceIndex: 4800 }]) {
      expect(() => contract.assertServerMessage(message(bad)), JSON.stringify(bad)).toThrow();
    }
  });
});
