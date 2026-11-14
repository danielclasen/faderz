import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { Ajv2020 } from 'ajv/dist/2020.js';
import addFormatsModule from 'ajv-formats';
import { parse as parseYaml } from 'yaml';
import { API_DIR, Contract } from '../../dev/mock/contract';

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
  ])('refuses %s = %j', (path, value) => {
    expect(set(path, value)).not.toBeNull();
  });
});
