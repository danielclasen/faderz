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

/**
 * Keys with a pressed or checked state that write nothing to the mixer
 * state, so they carry no pending mark: what they show is this screen's
 * own (selection, view, bank, FINE, the desk lock, the theme, the
 * loudness readout unit, the delay entry unit), a REST action that says in a notice when
 * it is refused (scene store and delete), or TALK, which starts nothing
 * while nothing commits (TalkKey). Each entry is a file and a fragment of
 * the key's tag found in exactly one key.
 */
const EXEMPT: ReadonlyArray<[file: string, fragment: string]> = [
  ['BusMaster.svelte', 'class="head"'],
  ['ChannelStrip.svelte', 'class="head"'],
  ['DelayEntry.svelte', 'aria-checked={unit === u.id}'],
  ['LockKey.svelte', 'class="key lock"'],
  ['MixerView.svelte', 'class="key sof-exit"'],
  ['MixerView.svelte', 'aria-pressed={visibleBank === i}'],
  ['MixerView.svelte', 'class="key spill"'],
  ['MixerView.svelte', 'class="key follow"'],
  ['MixerView.svelte', 'class="key fine"'],
  ['MixerView.svelte', 'class="key masters-key"'],
  ['ScenesView.svelte', 'class="pick"'],
  ['ScenesView.svelte', 'storeScene(mixer, s.id)'],
  ['ScenesView.svelte', 'deleteScene(mixer, s.id)'],
  ['SetupView.svelte', 'aria-checked={loudnessUnit.value === u}'],
  ['SetupView.svelte', 'aria-checked={theme.value === t.id}'],
  ['TalkKey.svelte', 'aria-pressed={active}'],
];

/** Keys with a pressed or checked state that carry no pending mark and are not exempt. */
function unmarked(file: string, src: string): string[] {
  return buttons(src)
    .filter((tag) => /aria-(pressed|checked)=/.test(tag) && !tag.includes('{@attach pendingWrite('))
    .filter((tag) => !EXEMPT.some(([f, frag]) => f === file && tag.includes(frag)))
    .map((tag) => `${file}: ${tag.replace(/\s+/g, ' ').slice(0, 100)}`);
}

describe('pending marks', () => {
  const files = readdirSync(DIR).filter((x) => x.endsWith('.svelte'));
  const src = (f: string) => readFileSync(join(DIR, f), 'utf8');

  it('every key with a pressed or checked state carries the pending mark, unless it is exempt', () => {
    expect(files.flatMap((f) => unmarked(f, src(f)))).toEqual([]);
  });

  it('every key that writes the state carries the pending mark, whatever its state', () => {
    const missing: string[] = [];
    let writing = 0;
    for (const f of files.filter((x) => x !== 'TalkKey.svelte')) {
      for (const tag of buttons(src(f))) {
        if (!WRITES.test(tag)) continue;
        writing++;
        if (!tag.includes('{@attach pendingWrite(')) missing.push(`${f}: ${tag.replace(/\s+/g, ' ').slice(0, 100)}`);
      }
    }
    expect(writing).toBeGreaterThan(40);
    expect(missing).toEqual([]);
  });

  it('names each exempt key once, so the list does not outlive its keys', () => {
    for (const [f, frag] of EXEMPT) expect(buttons(src(f)).filter((t) => t.includes(frag)), `${f} ${frag}`).toHaveLength(1);
  });

  it('catches a key whose write goes through another name, such as a hold', () => {
    const key = `<button class="key hold pause" aria-pressed={!running} use:hold={{ ms: 500, fire: running ? pause : run }}>PAUSE</button>`;
    expect(unmarked('Loudness.svelte', key)).toHaveLength(1);
    expect(unmarked('Loudness.svelte', key.replace('<button', '<button {@attach pendingWrite(mixer, runPath)}'))).toEqual([]);
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
