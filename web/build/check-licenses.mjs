// Fails unless dist/THIRD-PARTY-WEB.txt names every npm package the
// built UI's source maps draw from, and holds the font licence of the
// package each font file it ships comes from. Run after `npm run build`.

import { existsSync, readFileSync, readdirSync } from 'node:fs';

const notice = readFileSync('dist/THIRD-PARTY-WEB.txt', 'utf8');
const packages = new Set();
for (const file of readdirSync('dist/assets').filter((f) => f.endsWith('.map'))) {
  for (const source of JSON.parse(readFileSync(`dist/assets/${file}`, 'utf8')).sources) {
    const m = /node_modules\/((@[^/]+\/)?[^/]+)/.exec(source);
    if (m) packages.add(m[1]);
  }
}
const missing = [...packages].filter((p) => !notice.includes(`\n${p} `));
if (missing.length > 0) throw new Error(`THIRD-PARTY-WEB.txt misses ${missing.join(', ')}`);
// Fonts come through CSS, which the source maps do not list: each font
// file is traced to the package that ships it under that name, whose
// section of the notice has to hold its font licence.
const sections = new Map(
  notice.split(/^={80}$/m).slice(1).map((s) => [s.trim().split(' ')[0], s]),
);
for (const font of readdirSync('dist/assets').filter((f) => /\.(woff2?|ttf|otf)$/.test(f))) {
  const name = font.replace(/-[A-Za-z0-9_-]{8}(\.[a-z0-9]+)$/, '$1');
  const owner = readdirSync('node_modules/@fontsource')
    .map((p) => `@fontsource/${p}`)
    .find((p) => existsSync(`node_modules/${p}/files/${name}`));
  if (!owner) throw new Error(`no package ships ${font}`);
  if (!sections.get(owner)?.includes('SIL OPEN FONT LICENSE')) {
    throw new Error(`THIRD-PARTY-WEB.txt holds no font licence of ${owner} for ${font}`);
  }
  packages.add(owner);
}
console.log(`THIRD-PARTY-WEB.txt covers ${[...packages].sort().join(', ')}`);
