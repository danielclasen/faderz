// Vite plugin writing THIRD-PARTY-WEB.txt beside the built UI: the name,
// version, licence and licence text of every npm package with code,
// styles or fonts in the bundle. A bundled package without a licence
// field or licence file fails the build.

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { Plugin } from 'vite';

export const NOTICE = 'THIRD-PARTY-WEB.txt';

interface Package {
  name: string;
  version: string;
  license: string;
  text: string;
}

/** The package a file under node_modules belongs to: the nearest package.json with a name. */
function packageOf(file: string): string | null {
  const at = file.lastIndexOf('/node_modules/');
  if (at === -1) return null;
  let dir = dirname(file.split('?')[0]!);
  while (dir.length > at + '/node_modules'.length) {
    const manifest = join(dir, 'package.json');
    if (existsSync(manifest) && (JSON.parse(readFileSync(manifest, 'utf8')) as { name?: string }).name) return dir;
    dir = dirname(dir);
  }
  return null;
}

function read(dir: string): Package {
  const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) as { name: string; version: string; license?: string };
  const file = readdirSync(dir).find((f) => /^(licen[cs]e|copying)(\.(md|txt))?$/i.test(f));
  if (!manifest.license || !file) {
    throw new Error(`${manifest.name}@${manifest.version} is in the bundle without a licence field and file`);
  }
  return { name: manifest.name, version: manifest.version, license: manifest.license, text: readFileSync(join(dir, file), 'utf8').trim() };
}

export function render(packages: Package[]): string {
  const parts = packages.map((p) => `${'='.repeat(80)}\n${p.name} ${p.version} (${p.license})\n\n${p.text}\n`);
  return `Third-party npm packages in the faderz web UI and their licences.\n\n${parts.join('\n')}`;
}

export function licenses(): Plugin {
  return {
    name: 'faderz-licenses',
    apply: 'build',
    generateBundle(_options, bundle) {
      const dirs = new Set<string>();
      for (const chunk of Object.values(bundle)) {
        if (chunk.type !== 'chunk') continue;
        for (const id of chunk.moduleIds) {
          const dir = packageOf(id);
          if (dir) dirs.add(dir);
        }
      }
      const packages = [...dirs].map(read).sort((a, b) => a.name.localeCompare(b.name));
      this.emitFile({ type: 'asset', fileName: NOTICE, source: render(packages) });
    },
  };
}
