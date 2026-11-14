// Validators for the committed contract in ../../../api: the WebSocket
// JSON Schemas and the request and response bodies of openapi.yaml.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { Ajv2020, type ValidateFunction } from 'ajv/dist/2020.js';
import addFormatsModule from 'ajv-formats';
import { parse as parseYaml } from 'yaml';

const addFormats = addFormatsModule as unknown as (ajv: Ajv2020) => Ajv2020;

export const API_DIR = fileURLToPath(new URL('../../../api/', import.meta.url));
const ID_BASE = 'https://github.com/danielclasen/faderz/raw/main/api/';

export class ContractViolation extends Error {}

type OpenApiDoc = {
  paths: Record<string, Record<string, OpenApiOperation | unknown>>;
  components: { responses: Record<string, OpenApiResponse> };
};
type OpenApiOperation = {
  requestBody?: { content: Record<string, { schema: unknown }> };
  responses: Record<string, OpenApiResponse | { $ref: string }>;
};
type OpenApiResponse = { content?: Record<string, { schema: unknown }> };

function readJson(file: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(API_DIR, file), 'utf8')) as Record<string, unknown>;
}

function escape(token: string): string {
  return token.replace(/~/g, '~0').replace(/\//g, '~1');
}

function describe(v: ValidateFunction): string {
  return (v.errors ?? [])
    .slice(0, 5)
    .map((e) => `${e.instancePath || '/'} ${e.message ?? ''}`)
    .join('; ');
}

export class Contract {
  readonly ajv: Ajv2020;
  private readonly openapi: OpenApiDoc;
  private readonly cache = new Map<string, ValidateFunction | null>();
  readonly clientMessage: ValidateFunction;
  readonly serverMessage: ValidateFunction;
  /** Decoded meter frame values: arrays of ChannelMeter and BusMeter. */
  readonly meterValues: ValidateFunction;

  constructor() {
    // strict is off because openapi.yaml is registered whole and carries
    // OpenAPI keywords next to its schemas; the schema files compile under
    // strict mode in the contract tests.
    this.ajv = new Ajv2020({ strict: false, allErrors: true });
    addFormats(this.ajv);
    for (const f of ['model.schema.json', 'client-message.schema.json', 'server-message.schema.json']) {
      this.ajv.addSchema(readJson(`ws/${f}`));
    }
    this.openapi = parseYaml(readFileSync(join(API_DIR, 'openapi.yaml'), 'utf8')) as OpenApiDoc;
    this.ajv.addSchema({ ...(this.openapi as object), $id: `${ID_BASE}openapi.yaml` });
    this.clientMessage = this.ajv.getSchema(`${ID_BASE}ws/client-message.schema.json`)!;
    this.serverMessage = this.ajv.getSchema(`${ID_BASE}ws/server-message.schema.json`)!;
    const meter = (def: string) => ({ type: 'array', items: { $ref: `${ID_BASE}ws/model.schema.json#/$defs/${def}` } });
    this.meterValues = this.ajv.compile({
      type: 'object',
      required: ['channels', 'buses', 'auxes', 'mixMinus'],
      properties: { channels: meter('ChannelMeter'), buses: meter('BusMeter'), auxes: meter('BusMeter'), mixMinus: meter('BusMeter') },
    });
  }

  /** Throws when decoded meter values do not match model.schema.json ChannelMeter and BusMeter. */
  assertMeterValues(values: { channels: unknown[]; buses: unknown[]; auxes: unknown[]; mixMinus: unknown[] }): void {
    if (!this.meterValues(values)) {
      throw new ContractViolation(`meter frame values violate the contract: ${describe(this.meterValues)}`);
    }
  }

  checkClientMessage(msg: unknown): string | null {
    return this.clientMessage(msg) ? null : describe(this.clientMessage);
  }

  assertServerMessage(msg: unknown): void {
    if (!this.serverMessage(msg)) {
      throw new ContractViolation(`server message violates the contract: ${describe(this.serverMessage)}`);
    }
  }

  /** Validator of a schema at a JSON pointer inside openapi.yaml, or null. */
  private at(pointer: string): ValidateFunction | null {
    if (!this.cache.has(pointer)) {
      this.cache.set(pointer, this.ajv.compile({ $ref: `${ID_BASE}openapi.yaml#${pointer}` }));
    }
    return this.cache.get(pointer)!;
  }

  private operation(path: string, method: string): OpenApiOperation {
    const op = this.openapi.paths[path]?.[method.toLowerCase()] as OpenApiOperation | undefined;
    if (!op) throw new ContractViolation(`${method} ${path} is not in openapi.yaml`);
    return op;
  }

  /** Request body problems, or null when it matches. */
  checkRequest(path: string, method: string, body: unknown): string | null {
    const op = this.operation(path, method);
    if (!op.requestBody) return body === undefined ? null : 'operation takes no body';
    const v = this.at(`/paths/${escape(path)}/${method.toLowerCase()}/requestBody/content/application~1json/schema`)!;
    return v(body) ? null : describe(v);
  }

  /** Throws when a response status or body is not declared for the operation. */
  assertResponse(path: string, method: string, status: number, body: unknown): void {
    const op = this.operation(path, method);
    let response = op.responses[String(status)];
    let pointer = `/paths/${escape(path)}/${method.toLowerCase()}/responses/${status}`;
    if (!response) throw new ContractViolation(`${method} ${path} does not declare status ${status}`);
    if ('$ref' in response) {
      const name = response.$ref.replace('#/components/responses/', '');
      response = this.openapi.components.responses[name]!;
      pointer = `/components/responses/${escape(name)}`;
    }
    if (!response.content) {
      if (body !== undefined) throw new ContractViolation(`${method} ${path} ${status} declares no body`);
      return;
    }
    const v = this.at(`${pointer}/content/application~1json/schema`)!;
    if (!v(body)) {
      throw new ContractViolation(`${method} ${path} ${status} body violates the contract: ${describe(v)}`);
    }
  }
}
