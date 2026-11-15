// No width from 320 px up scrolls the page sideways, and every control
// of the header, the bank bar and the status bar is on screen with its
// text whole, with a mouse and with a finger, unlocked and with the desk
// locked. Runs on the desktop project, which sizes its own pages.

import { expect, test, type Browser, type Page } from '@playwright/test';
import { live, resetMock } from './support';

test.beforeEach(async ({ request }) => resetMock(request));

/** Phones, tablets in portrait and landscape, and the band between them. */
const SIZES = [
  [320, 568],
  [360, 740],
  [375, 667],
  [390, 844],
  [701, 900],
  [768, 1024],
  [820, 1180],
  [834, 1194],
  [900, 1000],
  [1023, 768],
  [1024, 1366],
  [1180, 820],
  [1399, 900],
] as const;

/** Problems at this size: sideways page scroll, or a bar control off screen or with its text cut. */
async function problems(page: Page): Promise<string[]> {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  return page.evaluate(() => {
    const out: string[] = [];
    const W = document.documentElement.clientWidth;
    if (document.documentElement.scrollWidth > W) out.push(`page ${document.documentElement.scrollWidth} > ${W}`);
    for (const el of document.querySelectorAll<HTMLElement>('header.top button, header.top select, .bankbar button, .bankbar select, .bankbar input, footer.bar button')) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 || getComputedStyle(el).visibility === 'hidden') continue;
      const name = `${el.tagName.toLowerCase()} "${(el.textContent ?? '').trim().slice(0, 16)}"`;
      if (r.left < -0.5 || r.right > W + 0.5) out.push(`${name} at ${Math.round(r.left)}..${Math.round(r.right)} of ${W}`);
      if (el.tagName === 'BUTTON' && el.scrollWidth > el.clientWidth + 1) out.push(`${name} text ${el.scrollWidth} > ${el.clientWidth}`);
    }
    return out;
  });
}

async function check(browser: Browser, baseURL: string, touch: boolean): Promise<string[]> {
  const found: string[] = [];
  const context = await browser.newContext({ baseURL, viewport: { width: 1280, height: 900 }, hasTouch: touch, isMobile: touch });
  const page = await context.newPage();
  try {
    await live(page);
    for (const [width, height] of SIZES) {
      await page.setViewportSize({ width, height });
      found.push(...(await problems(page)).map((p) => `${touch ? 'touch' : 'mouse'} ${width}x${height}: ${p}`));
    }
  } finally {
    await context.close();
  }
  return found;
}

test('no width from 320 px scrolls sideways or cuts a bar control, by mouse or by touch', async ({ browser, baseURL }) => {
  test.setTimeout(90_000);
  expect([...(await check(browser, baseURL!, false)), ...(await check(browser, baseURL!, true))]).toEqual([]);
});

test('a locked desk keeps its key on screen at every width from 320 px', async ({ page }) => {
  await live(page);
  const lock = page.getByTestId('desk-lock');
  const box = (await lock.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect(lock).toHaveAttribute('aria-pressed', 'true');
  const found: string[] = [];
  for (const [width, height] of SIZES) {
    await page.setViewportSize({ width, height });
    if (!(await lock.isVisible())) found.push(`${width}x${height}: no lock key`);
    found.push(...(await problems(page)).map((p) => `${width}x${height}: ${p}`));
  }
  expect(found).toEqual([]);
});
