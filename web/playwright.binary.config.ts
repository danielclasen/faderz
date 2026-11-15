// The UI and API served by the faderz binary itself (FADERZ_BIN), in each
// auth mode: none, token, and proxy behind a stand-in authenticating
// proxy (tests/binary/proxy.mjs). Build the UI and then the binary first.

import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
import { exampleShape } from './dev/mock/shape';

const BIN = process.env['FADERZ_BIN'];
if (!BIN) throw new Error('FADERZ_BIN names the faderz binary under test');
const BASE = Number(process.env['FADERZ_BINARY_PORT'] ?? 6471);
const DIR = resolve('node_modules/.cache/faderz-binary');
export const TOKEN = 'e2e-api-token-0123456789';
/** API port of each mode; ops and NMOS take the two after it. */
export const PORTS = { none: BASE, token: BASE + 10, proxy: BASE + 20 } as const;
/** The stand-in proxy in front of the proxy-mode API. */
export const PROXY_PORT = BASE + 24;

mkdirSync(DIR, { recursive: true });
writeFileSync(`${DIR}/api-token`, TOKEN);
const auth = {
  none: { mode: 'none' },
  token: { mode: 'token', token: { file: `${DIR}/api-token` } },
  proxy: { mode: 'proxy', proxy: { groupRoles: { operators: 'operator' } } },
};
for (const mode of ['none', 'token', 'proxy'] as const) {
  const port = PORTS[mode];
  const listener = (p: number) => ({ bind: '127.0.0.1', port: p });
  const origin = mode === 'proxy' ? `http://127.0.0.1:${PROXY_PORT}` : `http://127.0.0.1:${port}`;
  const config = {
    listeners: { api: listener(port), ops: listener(port + 1), nmos: listener(port + 2) },
    auth: auth[mode],
    api: { allowedOrigins: [origin] },
    replica: { name: `r-${mode}`, dataDir: `${DIR}/${mode}/data` },
    shape: exampleShape(),
  };
  writeFileSync(`${DIR}/${mode}.json`, JSON.stringify(config, null, 2));
}

const serve = (mode: keyof typeof PORTS) => ({
  command: `rm -rf '${DIR}/${mode}' && exec '${resolve(BIN)}' --config '${DIR}/${mode}.json'`,
  // Up once the API answers, with 401 where it wants credentials;
  // /readyz waits for an engine there is none of yet.
  url: `http://127.0.0.1:${PORTS[mode]}/api/v1/replicas`,
  reuseExistingServer: false,
});

export default defineConfig({
  testDir: 'tests/binary',
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  forbidOnly: !!process.env['CI'],
  reporter: process.env['CI'] ? [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-binary' }]] : [['list']],
  use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 }, trace: 'retain-on-failure' },
  projects: [
    // Each mode runs the tests tagged @all and its own.
    { name: 'none', grep: /@all|@none/, use: { baseURL: `http://127.0.0.1:${PORTS.none}` } },
    { name: 'token', grep: /@all|@token/, use: { baseURL: `http://127.0.0.1:${PORTS.token}` } },
    { name: 'proxy', grep: /@all|@proxy/, use: { baseURL: `http://127.0.0.1:${PROXY_PORT}` } },
  ],
  webServer: [
    serve('none'),
    serve('token'),
    serve('proxy'),
    {
      command: `node tests/binary/proxy.mjs ${PROXY_PORT} ${PORTS.proxy}`,
      port: PROXY_PORT,
      reuseExistingServer: false,
    },
  ],
});
