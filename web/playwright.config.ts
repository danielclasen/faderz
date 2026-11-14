import { networkInterfaces } from 'node:os';
import { defineConfig, devices } from '@playwright/test';

const PORT = 5181;
/** A LAN address of this host: a plain-HTTP origin is not a secure context. */
const LAN =
  Object.values(networkInterfaces())
    .flat()
    .find((i) => i && i.family === 'IPv4' && !i.internal)?.address ?? '127.0.0.1';
export const LAN_URL = `http://${LAN}:${PORT}`;

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
      name: 'desktop',
      testIgnore: /mobile\.spec\.ts/,
      use: { ...devices['Desktop Chrome'], viewport: { width: 1600, height: 1000 } },
    },
    // Phone descriptors on Chromium (the only browser installed), opened
    // through the LAN address so the page is not a secure context.
    { name: 'pixel-7', testMatch: /mobile\.spec\.ts/, use: { ...devices['Pixel 7'], baseURL: LAN_URL } },
    { name: 'iphone-14', testMatch: /mobile\.spec\.ts/, use: { ...devices['iPhone 14'], browserName: 'chromium', baseURL: LAN_URL } },
  ],
  webServer: {
    command: `npm run dev -- --host 0.0.0.0 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
    env: { FADERZ_MOCK_AUTOMATION: '0' },
  },
});
