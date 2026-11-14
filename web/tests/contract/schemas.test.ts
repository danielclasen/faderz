import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { Ajv2020 } from 'ajv/dist/2020.js';
import addFormatsModule from 'ajv-formats';
import { parse as parseYaml } from 'yaml';
import { API_DIR, Contract } from '../../dev/mock/contract';
import { CONTINUOUS_PATH_PATTERNS, isContinuousPath } from '../../src/lib/contract/continuous';

const addFormats = addFormatsModule as unknown as (ajv: Ajv2020) => Ajv2020;
const FILES = ['model.schema.json', 'client-message.schema.json', 'server-message.schema.json'];
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
    ['/channels/ch1/pan/divergence', 0.4],
    ['/channels/ch1/pan/lfe', -10],
    ['/channels/st1/image', 'leftToBoth'],
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
    ['/channels/ch1/pan/divergence', 1.5],
    ['/channels/ch1/pan/lfe', 12],
    ['/channels/st1/image', 'mono'],
    ['/buses/pgm20/image', 'swap'],
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
    '/channels/ch1/pan/lfe',
    '/channels/ch1/eq/bands/2/q',
    '/buses/pgm51/comp/makeup',
    '/auxes/aux1/delay',
    '/mixMinus/mm1/fader',
    '/vcas/vca1/offset',
    '/monitor/level',
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
  const owner = { principal: 'anonymous', session: 'tab-1', kind: 'ui', connection: 'c1' };
  const cmd = '0b6e3a4c-5d1f-4c2a-9e8b-7f6a5d4c3b2a';
  const cmd2 = '1c7f4b5d-6e2a-4d3b-8f9c-8a7b6e5d4c3b';

  it.each([
    { type: 'hello', session: 'tab-1', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'console:mcu-1', kind: 'console', mode: 'standby', lastRevision: 12 },
    { type: 'promote', lastRevision: 4 },
    { type: 'resync', lastRevision: 4, full: true },
    { type: 'credit', n: 1 },
    { type: 'credit', n: 8 },
    { type: 'credit', n: 100 },
    { type: 'ping', t: 0 },
    { type: 'pong', t: 1731590000000 },
    { type: 'resync' },
    { type: 'touch', id: cmd, path: '/channels/ch1/fader' },
    { type: 'release', id: cmd, path: '/channels/ch1/fader' },
  ])('client accepts %j', (msg) => {
    expect(contract.checkClientMessage(msg)).toBeNull();
  });

  it.each([
    { type: 'hello', clientId: 'tab-1', kind: 'ui', mode: 'primary' },
    { type: 'hello', session: 'tab-1', kind: 'ui' },
    { type: 'hello', session: 'tab-1', mode: 'primary' },
    { type: 'hello', session: 'tab-1', kind: 'script', mode: 'primary' },
    { type: 'hello', session: 'tab-1', kind: 'ui', mode: 'backup' },
    { type: 'hello', session: 'tab 1', kind: 'ui', mode: 'primary' },
    { type: 'promote', mode: 'primary' },
    { type: 'promote' },
    { type: 'resync', lastRevision: 4, full: false },
    { type: 'credit' },
    { type: 'credit', n: 0 },
    { type: 'credit', n: -1 },
    { type: 'credit', n: 1.5 },
    { type: 'credit', n: 1, channels: ['ch1'] },
    { type: 'ping' },
    { type: 'ping', t: -1 },
    { type: 'ping', t: 1, revision: 4 },
    { type: 'pong', t: 1, replica },
    { type: 'meters' },
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
      { owner: { ...owner, session: 'console:mcu-1', kind: 'console', connection: 'c2' }, commandId: cmd2 },
    ],
  };

  it.each([
    ['delta with several origins', delta],
    ['delta releasing ownership', { type: 'delta', replica, revision: 8, owners: [{ path: '/channels/ch1/fader', owner: null }] }],
    ['resume', { type: 'resume', you: owner, replica, revision: 7, deltas: [delta] }],
    ['script owner', { type: 'delta', replica, revision: 9, owners: [{ path: '/buses/pgm/fader', owner: { ...owner, kind: 'script' } }] }],
    ['replicas', { type: 'replicas', replica, replicas: [{ ...replica, healthy: true, commitable: true }, { id: 'r2', role: 'follower', healthy: true, commitable: false }] }],
    ['ping', { type: 'ping', replica, t: 0 }],
    ['pong', { type: 'pong', replica, t: 5 }],
    ['409 with owner', { type: 'error', replica, code: 409, message: 'owned', commandId: cmd, path: '/channels/ch1/fader', current: -3, owner }],
    ['counters', { type: 'counters', replica, values: { deadline_miss_total: 0 } }],
  ])('server accepts %s', (_, msg) => {
    expect(server(msg)).toBeNull();
  });

  it.each([
    ['meters', { type: 'meters', replica, channels: {}, buses: {}, auxes: {}, mixMinus: {} }],
    ['resume without you', { type: 'resume', replica, revision: 7, deltas: [] }],
    ['you as a client id', { type: 'resume', you: 'tab-1', replica, revision: 7, deltas: [] }],
    ['owner as a client id', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: 'tab-1' }] }],
    ['owner without connection', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { principal: 'anonymous', session: 'tab-1', kind: 'ui' } }] }],
    ['owner of unknown kind', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, kind: 'robot' } }] }],
    ['owner with empty principal', { ...delta, owners: [{ path: '/channels/ch1/fader', owner: { ...owner, principal: '' } }] }],
    ['origin without owner', { ...delta, origins: [{ commandId: cmd }] }],
    ['origin with seq 0', { ...delta, origins: [{ owner, commandId: cmd, seq: 0 }] }],
    ['empty origins', { ...delta, origins: [] }],
    ['replica without commitable', { type: 'replicas', replica, replicas: [{ ...replica, healthy: true }] }],
    ['ping with state', { type: 'ping', replica, t: 1, revision: 3 }],
    ['ping without replica', { type: 'ping', t: 1 }],
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
    expect(owners({ '/channels/ch1/fader': 'tab-1' })).toBe(false);
    expect(owners({ '/channels/ch1/peak': owner })).toBe(false);
  });
});
