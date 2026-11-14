// REST-backed operator actions. Each carries a fresh command id; RestClient
// retries a failed request with the same id on the next endpoint.

import type { CommandResult, Scene } from '../contract/types';
import { RestClient } from '../net/rest';
import { restBase } from '../sync/endpoints';
import { uuid } from '../net/ids';
import type { MixerStore } from './mixer.svelte';

let rest: RestClient | null = null;
let endpoints: readonly string[] = [];

export function configureRest(list: readonly string[], mixer: MixerStore): void {
  endpoints = list;
  rest = new RestClient(() => {
    const current = mixer.endpoint ?? endpoints[0]!;
    return [current, ...endpoints.filter((e) => e !== current)].map(restBase);
  });
}

function client(): RestClient {
  if (!rest) throw new Error('REST client not configured');
  return rest;
}

async function run<T>(mixer: MixerStore, what: string, fn: () => Promise<T>): Promise<T | null> {
  try {
    return await fn();
  } catch (e) {
    mixer.notify('error', `${what} failed: ${e instanceof Error ? e.message : String(e)}`);
    return null;
  }
}

export function recallScene(mixer: MixerStore, id: string, crossfadeMs: number) {
  return run(mixer, 'Scene recall', () =>
    client().request<CommandResult>('POST', `/scenes/${id}/recall`, { commandId: uuid(), crossfadeMs }),
  );
}

export function createScene(mixer: MixerStore, name: string) {
  return run(mixer, 'Scene store', () =>
    client().request<{ scene: Scene; result: CommandResult }>('POST', '/scenes', { commandId: uuid(), name }),
  );
}

export function storeScene(mixer: MixerStore, id: string, name?: string) {
  return run(mixer, 'Scene store', () =>
    client().request<{ scene: Scene; result: CommandResult }>('PUT', `/scenes/${id}`, {
      commandId: uuid(),
      ...(name ? { name } : {}),
    }),
  );
}

export function deleteScene(mixer: MixerStore, id: string) {
  return run(mixer, 'Scene delete', () => client().request<CommandResult>('DELETE', `/scenes/${id}`));
}

export function resetLoudness(mixer: MixerStore, ids: string[]) {
  return run(mixer, 'Loudness reset', () => client().request<null>('POST', '/loudness/reset', { ids }));
}
