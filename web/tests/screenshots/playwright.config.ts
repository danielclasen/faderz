// README screenshots: `npx playwright test -c tests/screenshots/playwright.config.ts`
// writes docs/screenshots/*.png from the show fixture in vite.config.ts.

import { defineConfig, devices } from '@playwright/test';

const PORT = Number(process.env['FADERZ_SCREENSHOT_PORT'] ?? 5191);

export default defineConfig({
  testDir: '.',
  workers: 1,
  timeout: 120_000,
  reporter: 'list',
  use: {
    ...devices['Desktop Chrome'],
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1600, height: 1000 },
    deviceScaleFactor: 2,
  },
  webServer: {
    command: `npx vite --config vite.config.ts --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
  },
});
