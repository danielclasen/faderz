import { expect, test, type Page } from '@playwright/test';
import { PERF_URL } from '../../playwright.config';
import { faderCenter, live, resetMock, serverState } from './support';

// Runs against the 64-channel mock server (playwright.config.ts). Tracing
// is off: its DOM snapshot after every action runs on the page's main thread.
test.use({ baseURL: PERF_URL, viewport: { width: 1600, height: 1000 }, trace: 'off' });

const WINDOW_MS = 5000;

interface Entry {
  start: number;
  duration: number;
}

/**
 * Records every main-thread long task and every patch measure from page
 * start. Only the window is judged: start-up work, such as the first
 * snapshot and the young-generation GC that follows it, runs once.
 */
async function observe(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { __longTasks: Entry[]; __patches: Entry[] };
    w.__longTasks = [];
    w.__patches = [];
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) w.__longTasks.push({ start: e.startTime, duration: e.duration });
    }).observe({ type: 'longtask', buffered: true });
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) if (e.name === 'faderz:patch') w.__patches.push({ start: e.startTime, duration: e.duration });
    }).observe({ type: 'measure', buffered: true });
  });
}

/** Moves a fader up and down with the pointer held until `until` (epoch ms). */
async function sweep(page: Page, path: string, until: number): Promise<number> {
  const { x, y } = await faderCenter(page, path);
  await page.mouse.move(x, y);
  await page.mouse.down();
  let moves = 0;
  while (Date.now() < until) {
    const dy = 40 * Math.sin(moves / 6);
    await page.mouse.move(x, y + dy);
    moves++;
  }
  await page.mouse.up();
  return moves;
}

test.beforeEach(async ({ request }) => resetMock(request));

test('three clients moving faders with meters on stay under 50 ms tasks and 4 ms patches', async ({ browser, request }) => {
  test.setTimeout(60_000);
  expect((await serverState(request)).shape.channels).toHaveLength(64);
  const contexts = await Promise.all([0, 1, 2].map(() => browser.newContext({ baseURL: PERF_URL, viewport: { width: 1600, height: 1000 } })));
  try {
    const pages = await Promise.all(contexts.map((c) => c.newPage()));
    for (const p of pages) await observe(p);
    await Promise.all(pages.map((p) => live(p)));
    await Promise.all(pages.map((p) => expect(p.getByTestId('meter-replica').locator('b')).toHaveText(/^r\d$/)));
    const starts = await Promise.all(pages.map((p) => p.evaluate(() => ({ t: performance.now(), patches: window.__faderz!.patches }))));
    const until = Date.now() + WINDOW_MS;
    const moves = await Promise.all(pages.map((p, i) => sweep(p, `/channels/ch${i + 1}/fader`, until)));
    for (const [i, p] of pages.entries()) {
      const r = await p.evaluate((t0) => {
        const w = window as unknown as { __longTasks: Entry[]; __patches: Entry[] };
        const inWindow = (e: Entry) => e.start >= t0;
        return {
          longTasks: w.__longTasks.filter(inWindow).map((e) => e.duration),
          allLongTasks: w.__longTasks.map((e) => e.duration),
          patchMs: w.__patches.filter(inWindow).map((e) => e.duration),
          patches: window.__faderz!.patches,
          maxPatchMs: window.__faderz!.maxPatchMs,
        };
      }, starts[i]!.t);
      const maxWindowPatch = Math.max(...r.patchMs);
      test.info().annotations.push({
        type: `page ${i + 1}`,
        description: JSON.stringify({ moves: moves[i], patches: r.patches - starts[i]!.patches, maxWindowPatch, maxPatchMs: r.maxPatchMs, longTasks: r.longTasks, loadLongTasks: r.allLongTasks }),
      });
      expect(moves[i]!).toBeGreaterThan(20);
      expect(r.patches - starts[i]!.patches).toBeGreaterThan(0);
      expect(r.patchMs.length).toBeGreaterThan(0);
      expect(r.longTasks).toEqual([]);
      expect(maxWindowPatch).toBeLessThan(4);
    }
  } finally {
    await Promise.all(contexts.map((c) => c.close()));
  }
});

test('a production build gives page scripts no session id in window.__faderz', async ({ page }) => {
  await live(page);
  const view = await page.evaluate(() => ({ keys: Object.keys(window.__faderz!), session: (window.__faderz as unknown as Record<string, unknown>)['session'] }));
  expect(view.keys).toContain('connection');
  expect(view.keys).not.toContain('session');
  expect(view.session).toBeUndefined();
});
