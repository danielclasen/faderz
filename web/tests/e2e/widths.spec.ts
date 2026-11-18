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

test('a tablet in portrait has the view tabs on a second header row, one tap each, and nothing moves', async ({ page }) => {
  test.setTimeout(60_000);
  // Layout shifts not caused by input, from the first paint on.
  await page.addInitScript(() => {
    const w = window as unknown as { __shifts: string[] };
    w.__shifts = [];
    new PerformanceObserver((list) => {
      for (const e of list.getEntries() as Array<PerformanceEntry & { value: number; hadRecentInput: boolean; sources?: Array<{ node?: Node }> }>) {
        if (!e.hadRecentInput && e.value > 0) w.__shifts.push(`${e.value.toFixed(4)} ${(e.sources ?? []).map((s) => (s.node as Element | undefined)?.className ?? '').join(',')}`);
      }
    }).observe({ type: 'layout-shift', buffered: true });
  });
  for (const [width, height] of [[701, 900], [768, 1024], [820, 1180], [1023, 1366]] as const) {
    await page.setViewportSize({ width, height });
    await live(page);
    const tabs = page.getByRole('navigation', { name: 'Views' }).getByRole('button');
    await expect(tabs).toHaveCount(7);
    await expect(page.getByRole('combobox', { name: 'View' })).toBeHidden();
    const header = (await page.locator('header.top').boundingBox())!;
    expect(header.height, `${width}x${height}`).toBe(76);
    for (const tab of await tabs.all()) {
      const box = (await tab.boundingBox())!;
      expect(box.x, `${width}x${height}`).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width, `${width}x${height}`).toBeLessThanOrEqual(width);
      expect(box.y, `${width}x${height}`).toBeGreaterThanOrEqual(44);
      expect(await tab.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
    }
    const main = (await page.locator('main').boundingBox())!;
    for (const name of ['Aux / N-1', 'Setup', 'Mixer']) {
      await page.getByRole('button', { name, exact: true }).click();
      await expect(page.getByRole('button', { name, exact: true })).toHaveAttribute('aria-current', 'page');
      expect(await page.locator('header.top').boundingBox()).toEqual(header);
      expect(await page.locator('main').boundingBox()).toEqual(main);
    }
    // A panel that opens under the header opens under both rows.
    await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
    const details = (await page.getByRole('region', { name: 'Connection details' }).boundingBox())!;
    expect(details.y).toBeGreaterThanOrEqual(header.y + header.height);
    await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
    expect(await page.evaluate(() => (window as unknown as { __shifts: string[] }).__shifts)).toEqual([]);
  }
  // In landscape at the same widths the views stay a menu on one row.
  await page.setViewportSize({ width: 1023, height: 768 });
  await expect(page.getByRole('combobox', { name: 'View' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Views' })).toBeHidden();
  expect((await page.locator('header.top').boundingBox())!.height).toBe(44);
});

test('rotating a tablet swaps the tab row and the menu, moves nothing else, and panels follow the header', async ({ page }) => {
  test.setTimeout(60_000);
  await page.addInitScript(() => {
    const w = window as unknown as { __shifts: number };
    w.__shifts = 0;
    new PerformanceObserver((list) => {
      for (const e of list.getEntries() as Array<PerformanceEntry & { value: number; hadRecentInput: boolean }>) if (!e.hadRecentInput) w.__shifts += e.value;
    }).observe({ type: 'layout-shift', buffered: true });
  });
  await page.setViewportSize({ width: 768, height: 1024 });
  await live(page);
  const header = page.locator('header.top');
  const tabs = page.getByRole('navigation', { name: 'Views' });
  const menu = page.getByRole('combobox', { name: 'View' });
  const settle = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  const shifts = () => page.evaluate(() => (window as unknown as { __shifts: number }).__shifts);
  for (const [width, height, rows] of [[1000, 768, 44], [768, 1024, 76], [1023, 768, 44], [834, 1194, 76]] as const) {
    await page.setViewportSize({ width, height });
    await settle();
    const box = (await header.boundingBox())!;
    expect(box.height, `${width}x${height}`).toBe(rows);
    await expect(rows === 76 ? tabs : menu).toBeVisible();
    await expect(rows === 76 ? menu : tabs).toBeHidden();
    // The header height every panel under it reads is the one drawn.
    const h = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--header-h').trim());
    expect(h, `${width}x${height}`).toBe(`${rows}px`);
    // After the rotation itself, nothing moves on its own.
    const after = await shifts();
    await page.waitForTimeout(1000);
    expect(await shifts(), `${width}x${height}`).toBe(after);
  }
});

test('the theme switches from the Setup view at every width, where the header has no theme key', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await live(page);
  await expect(page.getByRole('button', { name: /theme$/ })).toBeHidden();
  await page.getByRole('combobox', { name: 'View' }).selectOption('setup');
  const themes = page.getByRole('radiogroup', { name: 'Theme of this screen' });
  await expect(themes.getByRole('radio', { name: 'DARK' })).toHaveAttribute('aria-checked', 'true');
  await themes.getByRole('radio', { name: 'LIGHT' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await expect(themes.getByRole('radio', { name: 'LIGHT' })).toHaveAttribute('aria-checked', 'true');
  // Kept for this screen, and the header key agrees where it is shown.
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.setViewportSize({ width: 1600, height: 1000 });
  await expect(page.getByRole('button', { name: 'Switch to dark theme' })).toHaveText('DARK');
  await page.getByRole('button', { name: 'Switch to dark theme' }).click();
  await page.getByRole('button', { name: 'Setup', exact: true }).click();
  await expect(page.getByRole('radiogroup', { name: 'Theme of this screen' }).getByRole('radio', { name: 'DARK' })).toHaveAttribute('aria-checked', 'true');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
});

test('the duck tiles and the Duck view fit every width from 320 px, with no sideways page scroll and no tile text cut', async ({ page }) => {
  test.setTimeout(90_000);
  await live(page);
  const found: string[] = [];
  for (const [width, height] of SIZES) {
    await page.setViewportSize({ width, height });
    const masters = page.getByRole('button', { name: 'MASTERS' });
    const overlay = await masters.isVisible();
    if (overlay && (await masters.getAttribute('aria-pressed')) !== 'true') await masters.click();
    const tile = page.locator('[data-duck="comm"]');
    if ((await tile.count()) === 0) found.push(`${width}x${height}: no duck tile`);
    found.push(
      ...(await tile.evaluateAll((tiles) =>
        tiles.flatMap((t) =>
          [...t.querySelectorAll<HTMLElement>('button, [role="slider"], .lamp span, .thr')].flatMap((el) => {
            const r = el.getBoundingClientRect();
            const name = el.getAttribute('aria-label') ?? el.textContent ?? '';
            const out: string[] = [];
            if (r.left < -0.5 || r.right > document.documentElement.clientWidth + 0.5) out.push(`${name} at ${Math.round(r.left)}..${Math.round(r.right)}`);
            if (el.scrollWidth > el.clientWidth + 1) out.push(`${name} text ${el.scrollWidth} > ${el.clientWidth}`);
            return out;
          }),
        ),
      )).map((p) => `${width}x${height} tile: ${p}`),
    );
    found.push(...(await problems(page)).map((p) => `${width}x${height} mixer: ${p}`));
    if (overlay) await masters.click();
    const tab = page.getByRole('button', { name: 'Duck', exact: true });
    if (await tab.isVisible()) await tab.click();
    else await page.getByRole('combobox', { name: 'View' }).selectOption({ label: 'Duck' });
    await expect(page.getByRole('table', { name: 'Duck groups' })).toBeVisible();
    found.push(...(await problems(page)).map((p) => `${width}x${height} Duck view: ${p}`));
    if (await page.getByRole('button', { name: 'Mixer', exact: true }).isVisible()) await page.getByRole('button', { name: 'Mixer', exact: true }).click();
    else await page.getByRole('combobox', { name: 'View' }).selectOption({ label: 'Mixer' });
  }
  expect(found).toEqual([]);
});
