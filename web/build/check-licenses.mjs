// Fails unless dist/THIRD-PARTY-WEB.txt names every npm package the
// built UI's source maps draw from, and holds a licence text for the
// fonts it ships. Run after `npm run build`.

import { readFileSync, readdirSync } from 'node:fs';

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
if (readdirSync('dist/assets').some((f) => f.endsWith('.woff2')) && !notice.includes('SIL OPEN FONT LICENSE')) {
  throw new Error('THIRD-PARTY-WEB.txt holds no font licence for the fonts in the bundle');
}
console.log(`THIRD-PARTY-WEB.txt covers ${[...packages].sort().join(', ')}`);
