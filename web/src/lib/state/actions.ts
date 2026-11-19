// REST-backed operator actions. Each carries a fresh command id; RestClient
// retries a failed request with the same id on the next endpoint.

import type { About, Change, CommandResult, Scene } from '../contract/types';
import { RestClient } from '../net/rest';
import { restBase } from '../sync/endpoints';
import { uuid } from '../net/ids';
import type { MixerStore } from './mixer.svelte';

let rest: RestClient | null = null;
let endpoints: readonly string[] = [];

/** `onUnauthorized` runs when a request is answered 401. */
export function configureRest(list: readonly string[], mixer: MixerStore, onUnauthorized?: () => void): void {
  endpoints = list;
  rest = new RestClient(
    () => {
      const current = mixer.endpoint ?? endpoints[0]!;
      return [current, ...endpoints.filter((e) => e !== current)].map(restBase);
    },
    undefined,
    undefined,
    undefined,
    onUnauthorized,
  );
}

function client(): RestClient {
  if (!rest) throw new Error('REST client not configured');
  return rest;
}

async function run<T>(mixer: MixerStore, what: string, fn: () => Promise<T>): Promise<T | null> {
  if (mixer.locked) {
    mixer.notify('error', `${what} not sent: ${mixer.lockReason ?? 'the desk is locked'}`);
    return null;
  }
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

/** A retry after a lost answer resets once more: the measurement restarts either way. */
export function resetLoudness(mixer: MixerStore, ids: string[]) {
  return run(mixer, 'Loudness reset', () => client().request<null>('POST', '/loudness/reset', { ids }));
}

/** Writes `changes` in one revision, or none of them (PATCH /state), as a preset does. */
export function patchState(mixer: MixerStore, what: string, changes: Change[]) {
  return run(mixer, what, () => client().request<CommandResult>('PATCH', '/state', { commandId: uuid(), changes }));
}

/** What the serving replica was built from and its changelog; a read, so a locked desk asks it too. */
export function fetchAbout(): Promise<About> {
  return client().request<About>('GET', '/about');
}
