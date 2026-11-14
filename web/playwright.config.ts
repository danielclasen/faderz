import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { defineConfig, devices } from '@playwright/test';

const PORT = 5181;
/** A LAN address of this host: a plain-HTTP origin is not a secure context. */
const LAN =
  Object.values(networkInterfaces())
    .flat()
    .find((i) => i && i.family === 'IPv4' && !i.internal)?.address ?? '127.0.0.1';
export const LAN_URL = `http://${LAN}:${PORT}`;

/** Third dev server with 64 input channels, for the load spec. */
const PERF_PORT = 5183;
export const PERF_URL = `http://127.0.0.1:${PERF_PORT}`;

/** Second dev server over HTTPS with a throwaway self-signed certificate. */
const TLS_PORT = 5182;
const TLS_DIR = 'node_modules/.cache/faderz-e2e-tls';
if (!existsSync(`${TLS_DIR}/cert.pem`)) {
  mkdirSync(TLS_DIR, { recursive: true });
  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30', '-subj', '/CN=localhost',
    '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
    '-keyout', `${TLS_DIR}/key.pem`, '-out', `${TLS_DIR}/cert.pem`,
  ], { stdio: 'ignore' });
}

export default defineConfig({
  testDir: 'tests/e2e',
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: 'retain-on-failure',
  },
  projects: [
    {
      // perf.spec.ts runs here only, against the PERF_PORT server.
      name: 'desktop',
      testIgnore: /(mobile|https)\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 } },
    },
    // Phone descriptors on Chromium (the only browser installed), opened
    // through the LAN address so the page is not a secure context.
    { name: 'pixel-7', testMatch: /mobile\.spec\.ts/, use: { ...devices['Pixel 7'], baseURL: LAN_URL } },
    {
      name: 'https',
      testMatch: /https\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], baseURL: `https://127.0.0.1:${TLS_PORT}`, ignoreHTTPSErrors: true },
    },
    { name: 'iphone-14', testMatch: /mobile\.spec\.ts/, use: { ...devices['iPhone 14'], browserName: 'chromium', baseURL: LAN_URL } },
  ],
  webServer: [
    {
      command: `npm run dev -- --host 0.0.0.0 --port ${PORT} --strictPort`,
      url: `http://127.0.0.1:${PORT}/`,
      reuseExistingServer: false,
      env: { FADERZ_MOCK_AUTOMATION: '0' },
    },
    {
      command: `npm run dev -- --host 127.0.0.1 --port ${TLS_PORT} --strictPort`,
      url: `https://127.0.0.1:${TLS_PORT}/`,
      ignoreHTTPSErrors: true,
      reuseExistingServer: false,
      env: { FADERZ_MOCK_AUTOMATION: '0', FADERZ_DEV_TLS_CERT: `${TLS_DIR}/cert.pem`, FADERZ_DEV_TLS_KEY: `${TLS_DIR}/key.pem` },
    },
    {
      command: `npm run dev -- --host 127.0.0.1 --port ${PERF_PORT} --strictPort`,
      url: `${PERF_URL}/`,
      reuseExistingServer: false,
      env: { FADERZ_MOCK_AUTOMATION: '0', FADERZ_MOCK_CHANNELS: '64' },
    },
  ],
});
