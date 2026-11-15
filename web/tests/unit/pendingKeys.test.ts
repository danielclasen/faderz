// @vitest-environment happy-dom
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import SetupView from '../../src/components/SetupView.svelte';
import { INITIAL_STATUS, MixerStore, mixerContext } from '../../src/lib/state/mixer.svelte';
import type { SyncStatus } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';

const DIR = join(__dirname, '../../src/components');
/** A press on such a key writes the state. */
const WRITES = /mixer\.set\(|\btoggle(Solo|Target|Safe|Vca)?\(|\binvertAll\b/;
/** TALK is a momentary key: it starts nothing while nothing commits (TalkKey). */
const EXEMPT = new Set(['TalkKey.svelte']);

/** Opening tags of the buttons in a component, braces balanced. */
function buttons(src: string): string[] {
  const out: string[] = [];
  for (let i = src.indexOf('<button'); i >= 0; i = src.indexOf('<button', i + 1)) {
    let depth = 0;
    let k = i + 7;
    for (; k < src.length; k++) {
      const ch = src[k];
      if (ch === '{') depth++;
      else if (ch === '}') depth--;
      else if (ch === '>' && depth === 0 && src[k - 1] !== '=') break;
    }
    out.push(src.slice(i, k + 1));
  }
  return out;
}

describe('pending marks', () => {
  it('every key that writes the state carries the pending mark', () => {
    const missing: string[] = [];
    let writing = 0;
    for (const f of readdirSync(DIR).filter((x) => x.endsWith('.svelte') && !EXEMPT.has(x))) {
      for (const tag of buttons(readFileSync(join(DIR, f), 'utf8'))) {
        if (WRITES.test(tag)) writing++;
        if (WRITES.test(tag) && !tag.includes('{@attach pendingWrite(')) missing.push(`${f}: ${tag.replace(/\s+/g, ' ').slice(0, 100)}`);
      }
    }
    expect(writing).toBeGreaterThan(40);
    expect(missing).toEqual([]);
  });

  it('marks a key while its write is not committed and nothing commits, and only then', () => {
    const s = new MixerStore();
    s.attach({ post: () => {} });
    const shape = exampleShape();
    const status = (commitable: boolean, pendingKeys: string[]): SyncStatus => ({
      ...INITIAL_STATUS,
      link: 'Live',
      primaryReplica: 'r1',
      replicas: [{ id: 'r1', role: 'leader', healthy: true, commitable }],
      pendingKeys,
    });
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: status(true, []) });
    const target = document.createElement('div');
    document.body.append(target);
    const c = mount(SetupView, { target, context: mixerContext(s) });
    flushSync();
    const tone = target.querySelector<HTMLElement>('button[aria-label="Line-up tone"]')!;
    const ident = [...target.querySelectorAll<HTMLElement>('button')].find((b) => b.textContent === 'IDENT')!;
    expect(tone.hasAttribute('data-pending')).toBe(false);
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status(false, ['/tone/on']) });
    flushSync();
    expect(tone.hasAttribute('data-pending')).toBe(true);
    expect(ident.hasAttribute('data-pending')).toBe(false);
    s.apply({ type: 'patch', input: 0, changes: [], owners: [], status: status(true, ['/tone/on']) });
    flushSync();
    expect(tone.hasAttribute('data-pending')).toBe(false);
    unmount(c);
  });
});
