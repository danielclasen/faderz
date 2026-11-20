// Global setup of a run against the faderz backend (FADERZ_E2E_BACKEND):
// writes the mock's example state into the backend's empty state over
// REST, so the specs find the state they were written against.

import { exampleShape, initialState } from '../../dev/mock/shape';
import { pointer } from '../../src/lib/state/pointer';
import { BACKEND_URL } from '../../playwright.config';

const HEADERS = { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' };
/** Changes per PATCH: one command stays well within what the backend orders in one entry. */
const CHUNK = 100;
/** Parts of the state only the server writes, or that this setup writes otherwise. */
const SERVER_SIDE = /^\/(tally|scenes|consoles|sceneRecalled)(\/|$)|^\/afv\/[^/]+\/open$|^\/loudness\/[^/]+\/resets$/;

/** Writable leaves: an array of strings (an id set) is one leaf, other arrays one per leg. */
function leaves(value: unknown, base: string, out: Array<{ path: string; value: unknown }>): void {
  if (Array.isArray(value) && value.every((v) => typeof v === 'string')) out.push({ path: base, value });
  else if (Array.isArray(value)) value.forEach((v, i) => leaves(v, `${base}/${i}`, out));
  else if (value !== null && typeof value === 'object') {
    for (const [k, v] of Object.entries(value)) leaves(v, `${base}${pointer(k)}`, out);
  } else out.push({ path: base, value });
}

async function send(method: string, path: string, body: unknown): Promise<void> {
  const res = await fetch(`${BACKEND_URL}/api/v1${path}`, { method, headers: HEADERS, body: JSON.stringify(body) });
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
}

export default async function setup(): Promise<void> {
  const state = initialState(exampleShape());
  const changes: Array<{ path: string; value: unknown }> = [];
  leaves(state, '', changes);
  const writable = changes.filter((c) => !SERVER_SIDE.test(c.path));
  for (let i = 0; i < writable.length; i += CHUNK) {
    await send('PATCH', '/state', { commandId: crypto.randomUUID(), changes: writable.slice(i, i + CHUNK) });
  }
  await send('PUT', '/afv/tally', { sources: state.tally });
  for (const name of ['Opening', 'Interview']) await send('POST', '/scenes', { commandId: crypto.randomUUID(), name });
}
