import { writeFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';
import { PERF_URL } from '../../playwright.config';
import { RawClient, faderCenter, live, resetMock, serverState } from './support';

// Runs against the 64-channel mock server (playwright.config.ts). Tracing
// is off: its DOM snapshot after every action runs on the page's main thread.
test.use({ baseURL: PERF_URL, viewport: { width: 1600, height: 1000 }, trace: 'off' });

const WINDOW_MS = 5000;
/** Bound on the 99th percentile of patch time in the window, ms. */
const PATCH_P99_MS = 4;
/** No single patch may take a whole frame at 60 Hz, ms. */
const PATCH_CEILING_MS = 16;

/** Nearest-rank percentile of `xs`, 0 < q <= 1. */
function percentile(xs: readonly number[], q: number): number {
  const sorted = [...xs].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(q * sorted.length) - 1)]!;
}

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

// A failed attempt runs once more with a performance trace, which the
// first attempt runs without: tracing costs time on the page and could
// cause or hide a long task. The retry does not make the test pass:
// failOnFlakyTests fails the run either way.
test.describe.configure({ retries: 1 });

test('three clients moving faders with meters on and duck groups firing stay under 50 ms tasks, 4 ms patches at p99 and one frame per patch', { tag: '@mock' }, async ({ browser, request }, info) => {
  test.setTimeout(60_000);
  expect((await serverState(request)).shape.channels).toHaveLength(64);
  const contexts = await Promise.all([0, 1, 2].map(() => browser.newContext({ baseURL: PERF_URL, viewport: { width: 1600, height: 1000 } })));
  let tracing = false;
  let traced: Buffer | null = null;
  const stopTracing = async (): Promise<Buffer | null> => {
    if (tracing && !traced) traced = await browser.stopTracing();
    return traced;
  };
  try {
    const pages = await Promise.all(contexts.map((c) => c.newPage()));
    for (const p of pages) await observe(p);
    await Promise.all(pages.map((p) => live(p)));
    await Promise.all(pages.map((p) => expect(p.getByTestId('meter-replica').locator('b')).toHaveText(/^r\d$/)));
    // A console holds and lets go of every duck group each second; each
    // page counts the changes of a group's lamp.
    for (const p of pages) {
      await p.evaluate(() => {
        const w = window as unknown as { __duckChanges: number };
        w.__duckChanges = 0;
        new MutationObserver(() => w.__duckChanges++).observe(document.querySelector('[data-testid="duck-lamp-comm"]')!, { subtree: true, characterData: true, childList: true });
      });
    }
    expect((await request.post('/__mock/ducks/script?ms=1000')).ok()).toBe(true);
    // On a retry, a performance trace of the window, kept only when a
    // page has a long task in it, shows what ran in that task.
    if (info.retry > 0) {
      // Invalidation tracking with stacks names the element and the code
      // that made a layout or style recalculation as large as it was.
      await browser.startTracing(undefined, {
        categories: [
          'devtools.timeline',
          'disabled-by-default-devtools.timeline',
          'disabled-by-default-devtools.timeline.invalidationTracking',
          'disabled-by-default-devtools.timeline.stack',
          'v8.execute',
          'blink.user_timing',
        ],
      });
      tracing = true;
    }
    const starts = await Promise.all(pages.map((p) => p.evaluate(() => ({ t: performance.now(), patches: window.__faderz!.patches }))));
    const until = Date.now() + WINDOW_MS;
    const moves = await Promise.all(pages.map((p, i) => sweep(p, `/channels/ch${i + 1}/fader`, until)));
    await request.post('/__mock/ducks/script?ms=0');
    await stopTracing();
    for (const p of pages) expect(await p.evaluate(() => (window as unknown as { __duckChanges: number }).__duckChanges)).toBeGreaterThanOrEqual(3);
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
      const p99WindowPatch = percentile(r.patchMs, 0.99);
      test.info().annotations.push({
        type: `page ${i + 1}`,
        description: JSON.stringify({ moves: moves[i], patches: r.patches - starts[i]!.patches, p99WindowPatch, maxWindowPatch, maxPatchMs: r.maxPatchMs, longTasks: r.longTasks, loadLongTasks: r.allLongTasks }),
      });
      expect(moves[i]!).toBeGreaterThan(20);
      expect(r.patches - starts[i]!.patches).toBeGreaterThan(0);
      expect(r.patchMs.length).toBeGreaterThan(0);
      if (r.longTasks.length > 0 && tracing) {
        // Tens of MB: written to the test's output directory, which CI
        // uploads on failure, and not copied into the report.
        await writeFile(test.info().outputPath('performance-trace.json'), (await stopTracing())!);
      }
      expect(r.longTasks).toEqual([]);
      // A patch is judged by wall time, which on a shared runner includes
      // time the main thread is runnable but not scheduled: the tail is
      // judged by its 99th percentile, and every patch by one frame.
      expect(p99WindowPatch).toBeLessThan(PATCH_P99_MS);
      expect(maxWindowPatch).toBeLessThan(PATCH_CEILING_MS);
    }
  } finally {
    await request.post('/__mock/ducks/script?ms=0');
    // A test that fails inside the window leaves no trace running.
    await stopTracing().catch(() => undefined);
    await Promise.all(contexts.map((c) => c.close()));
  }
});

interface TraceEvent {
  name: string;
  ph: string;
  args?: { beginData?: { partialLayout?: boolean; totalObjects?: number } };
}

// A layout from the document down walks every strip of the console, tens
// of milliseconds on a loaded runner: the test above saw it as a long task
// whenever another client's touch showed or hid a fader's owner label or
// changed the header's user count. Each is laid out in a box of its own,
// so a touch and a release lay out those boxes, not the page.
test('another client touching and releasing a fader lays out no more than its boxes', { tag: '@mock' }, async ({ page, browser }) => {
  await live(page);
  const other = await RawClient.open(PERF_URL);
  const owner = page.locator('[data-path="/channels/ch2/fader"] [data-testid="owner"]');
  const frames = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  let tracing = false;
  try {
    await frames();
    await browser.startTracing(page, { categories: ['devtools.timeline', 'disabled-by-default-devtools.timeline'] });
    tracing = true;
    for (let i = 0; i < 3; i++) {
      await other.command({ type: 'touch', path: '/channels/ch2/fader' });
      await expect(owner).toBeVisible();
      await frames();
      await other.command({ type: 'release', path: '/channels/ch2/fader' });
      await expect(owner).toHaveCount(0);
      await frames();
    }
    tracing = false;
    const events = (JSON.parse((await browser.stopTracing()).toString()) as { traceEvents: TraceEvent[] }).traceEvents;
    const layouts = events.filter((e) => e.name === 'Layout' && e.ph === 'X');
    expect(layouts.length).toBeGreaterThan(0);
    // Every layout says whether it started below the document: without
    // that the check below would hold of any trace.
    expect(layouts.filter((e) => typeof e.args?.beginData?.partialLayout !== 'boolean')).toEqual([]);
    // Objects of each layout from the document down.
    expect(layouts.filter((e) => e.args?.beginData?.partialLayout === false).map((e) => e.args?.beginData?.totalObjects)).toEqual([]);
  } finally {
    if (tracing) await browser.stopTracing().catch(() => undefined);
    other.close();
  }
});

test('a production build gives page scripts no session id in window.__faderz', { tag: '@mock' }, async ({ page }) => {
  await live(page);
  const view = await page.evaluate(() => ({ keys: Object.keys(window.__faderz!), session: (window.__faderz as unknown as Record<string, unknown>)['session'] }));
  expect(view.keys).toContain('connection');
  expect(view.keys).not.toContain('session');
  expect(view.session).toBeUndefined();
});
