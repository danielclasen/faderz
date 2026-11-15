import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const SRC = join(__dirname, '../../src');

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? sources(join(dir, e.name)) : /\.(css|svelte)$/.test(e.name) ? [join(dir, e.name)] : [],
  );
}

describe('header layouts', () => {
  // The header's layout and the --header-h the panels under it read are
  // set in one media block each, so a breakpoint cannot change one alone.
  it('writes the portrait-tablet query and every --header-h value in App.svelte only', () => {
    const portrait: string[] = [];
    const height: string[] = [];
    for (const f of sources(SRC)) {
      const text = readFileSync(f, 'utf8');
      const name = f.slice(SRC.length + 1);
      for (const _ of text.matchAll(/orientation:\s*portrait/g)) portrait.push(name);
      for (const _ of text.matchAll(/--header-h\s*:/g)) height.push(name);
    }
    expect(portrait).toEqual(['App.svelte']);
    expect(height).toEqual(['App.svelte', 'App.svelte', 'App.svelte']);
  });
});
