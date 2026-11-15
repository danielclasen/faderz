import { readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { licenses } from './build/licenses';

// Dev-only HTTPS: FADERZ_DEV_TLS_CERT and FADERZ_DEV_TLS_KEY name PEM files.
// Without them `npm run dev` serves plain HTTP.
const tlsCert = process.env['FADERZ_DEV_TLS_CERT'];
const tlsKey = process.env['FADERZ_DEV_TLS_KEY'];
const https = tlsCert && tlsKey ? { cert: readFileSync(tlsCert), key: readFileSync(tlsKey) } : undefined;

// FADERZ_BACKEND names a faderz server, e.g. http://127.0.0.1:8080: dev
// serving passes /api/v1 to it, WebSocket included, in place of the mock.
const backend = process.env['FADERZ_BACKEND'];

export default defineConfig(async ({ command, mode }) => ({
  plugins: [
    svelte(),
    // The licence notice of every npm package in the bundle.
    licenses(),
    // The mock server exists only in `vite` dev serving; builds never load it.
    ...(command === 'serve' && mode !== 'test' && !backend ? [(await import('./dev/mock/plugin.ts')).faderzMock()] : []),
  ],
  // Component tests mount Svelte's client runtime, not its server one.
  ...(process.env['VITEST'] ? { resolve: { conditions: ['browser'] } } : {}),
  server: {
    fs: { allow: ['..'] },
    ...(https ? { https } : {}),
    ...(backend ? { proxy: { '/api/v1': { target: backend, ws: true } } } : {}),
  },
  build: {
    target: 'es2022',
    sourcemap: true,
  },
  test: {
    include: ['tests/unit/**/*.test.ts', 'tests/contract/**/*.test.ts'],
    environment: 'node',
  },
}));
