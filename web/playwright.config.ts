import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { networkInterfaces } from 'node:os';
import { resolve } from 'node:path';
import { defineConfig, devices } from '@playwright/test';
import { exampleShape } from './dev/mock/shape';

/** Plain HTTP dev server; the HTTPS and preview servers take the next two ports. */
const PORT = Number(process.env['FADERZ_E2E_PORT'] ?? 5181);
/** A LAN address of this host: a plain-HTTP origin is not a secure context. */
const LAN =
  Object.values(networkInterfaces())
    .flat()
    .find((i) => i && i.family === 'IPv4' && !i.internal)?.address ?? '127.0.0.1';
export const LAN_URL = `http://${LAN}:${PORT}`;

/**
 * Production build with 64 input channels under `vite preview`, which also
 * serves the mock, for the load spec: dev mode adds main-thread work that
 * a deployed client does not do.
 */
const PERF_PORT = PORT + 2;
const PERF_DIST = 'node_modules/.cache/faderz-e2e-perf-dist';
export const PERF_URL = `http://127.0.0.1:${PERF_PORT}`;

/** Second dev server over HTTPS with a throwaway self-signed certificate. */
const TLS_PORT = PORT + 1;
const TLS_DIR = 'node_modules/.cache/faderz-e2e-tls';
if (!existsSync(`${TLS_DIR}/cert.pem`)) {
  mkdirSync(TLS_DIR, { recursive: true });
  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '30', '-subj', '/CN=localhost',
    '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
    '-keyout', `${TLS_DIR}/key.pem`, '-out', `${TLS_DIR}/cert.pem`,
  ], { stdio: 'ignore' });
}

/**
 * FADERZ_E2E_BACKEND names a faderz binary: the suite then runs against
 * one replica of it behind the dev servers in place of the mock, with the
 * mock's example shape and, from the global setup, its example state.
 * Tests tagged @mock need the mock's controls or its several replicas,
 * @meters its meter frames; both are left out then.
 */
const BACKEND = process.env['FADERZ_E2E_BACKEND'];
const BACKEND_PORT = PORT + 3;
export const BACKEND_URL = `http://127.0.0.1:${BACKEND_PORT}`;
const BACKEND_DIR = resolve('node_modules/.cache/faderz-e2e-backend');
if (BACKEND) {
  mkdirSync(BACKEND_DIR, { recursive: true });
  const listener = (port: number) => ({ bind: '127.0.0.1', port });
  const config = {
    listeners: { api: listener(BACKEND_PORT), ops: listener(PORT + 4), nmos: listener(PORT + 5) },
    api: {
      allowedHosts: [LAN],
      allowedOrigins: [`http://127.0.0.1:${PORT}`, `http://localhost:${PORT}`, LAN_URL, `https://127.0.0.1:${PORT + 1}`],
    },
    replica: { name: 'r1', dataDir: `${BACKEND_DIR}/data` },
    shape: exampleShape(),
  };
  writeFileSync(`${BACKEND_DIR}/config.json`, JSON.stringify(config, null, 2));
}
const NOT_ON_BACKEND = BACKEND ? { grepInvert: /@mock|@meters/ } : {};

export default defineConfig({
  testDir: 'tests/e2e',
  ...(BACKEND ? { globalSetup: './tests/e2e/backend-setup.ts' } : {}),
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  forbidOnly: !!process.env['CI'],
  // A test that passes only on a retry fails the run: a retry exists to
  // collect what the first attempt could not (perf.spec's trace).
  failOnFlakyTests: true,
  // CI keeps an HTML report to upload when a test fails.
  reporter: process.env['CI'] ? [['list'], ['html', { open: 'never' }]] : [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: 'retain-on-failure',
  },
  projects: [
    {
      // perf.spec.ts runs here only, against the PERF_PORT server.
      name: 'desktop',
      ...NOT_ON_BACKEND,
      testIgnore: /(mobile|https|narrow|short)\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 } },
    },
    // Phone descriptors on Chromium (the only browser installed), opened
    // through the LAN address so the page is not a secure context.
    { name: 'pixel-7', ...NOT_ON_BACKEND, testMatch: /(mobile|layout)\.spec\.ts/, use: { ...devices['Pixel 7'], baseURL: LAN_URL } },
    {
      name: 'https',
      ...NOT_ON_BACKEND,
      testMatch: /https\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], baseURL: `https://127.0.0.1:${TLS_PORT}`, ignoreHTTPSErrors: true },
    },
    { name: 'iphone-14', ...NOT_ON_BACKEND, testMatch: /(mobile|layout)\.spec\.ts/, use: { ...devices['iPhone 14'], browserName: 'chromium', baseURL: LAN_URL } },

    // Below 1400 px the masters lie over the strips; below 500 px of
    // height the header and the strips are compact.
    { name: 'tablet', ...NOT_ON_BACKEND, testMatch: /(layout|narrow)\.spec\.ts/, use: { ...devices['iPad Pro 11 landscape'], browserName: 'chromium' } },

    {
      name: 'iphone-14-landscape',
      ...NOT_ON_BACKEND,
      testMatch: /(layout|narrow|short)\.spec\.ts/,
      use: { ...devices['iPhone 14 landscape'], browserName: 'chromium', baseURL: LAN_URL },
    },
  ],
  webServer: [
    ...(BACKEND
      ? [
          {
            // A fresh data directory: every run starts from an empty state.
            command: `rm -rf '${BACKEND_DIR}/data' && exec '${resolve(BACKEND)}' --config '${BACKEND_DIR}/config.json'`,
            url: `${BACKEND_URL}/api/v1/replicas`,
            reuseExistingServer: false,
          },
        ]
      : []),
    {
      command: `npm run dev -- --host 0.0.0.0 --port ${PORT} --strictPort`,
      url: `http://127.0.0.1:${PORT}/`,
      reuseExistingServer: false,
      env: { FADERZ_MOCK_AUTOMATION: '0', ...(BACKEND ? { FADERZ_BACKEND: BACKEND_URL } : {}) },
    },
    {
      command: `npm run dev -- --host 127.0.0.1 --port ${TLS_PORT} --strictPort`,
      url: `https://127.0.0.1:${TLS_PORT}/`,
      ignoreHTTPSErrors: true,
      reuseExistingServer: false,
      env: {
        FADERZ_MOCK_AUTOMATION: '0',
        FADERZ_DEV_TLS_CERT: `${TLS_DIR}/cert.pem`,
        FADERZ_DEV_TLS_KEY: `${TLS_DIR}/key.pem`,
        ...(BACKEND ? { FADERZ_BACKEND: BACKEND_URL } : {}),
      },
    },
    // The load spec measures against the mock's meters.
    ...(BACKEND ? [] : [{
      command: `npx vite build --outDir ${PERF_DIST} --emptyOutDir && npx vite preview --outDir ${PERF_DIST} --host 127.0.0.1 --port ${PERF_PORT} --strictPort`,
      url: `${PERF_URL}/`,
      timeout: 180_000,
      reuseExistingServer: false,
      env: { FADERZ_MOCK_AUTOMATION: '0', FADERZ_MOCK_CHANNELS: '64' },
    }]),
  ],
});
