// No width from 320 px up scrolls the page sideways, and every control
// of the header, the bank bar and the status bar is on screen with its
// text whole, with a mouse and with a finger, unlocked and with the desk
// locked. Runs on the desktop project, which sizes its own pages.

import { expect, test, type Browser, type Page } from '@playwright/test';
import { live, resetMock, switchTheme } from './support';

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

test('the header has no theme key at any width; the account menu and the Setup view switch the theme', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 });
  await live(page);
  for (const [width, height] of [...SIZES, [1600, 1000]] as const) {
    await page.setViewportSize({ width, height });
    await expect(page.locator('header.top').getByRole('button', { name: /theme$/ }), `${width}x${height}`).toHaveCount(0);
  }
  // A phone: the account pill's menu switches it.
  await page.setViewportSize({ width: 360, height: 740 });
  await switchTheme(page, 'light');
  await page.getByRole('combobox', { name: 'View' }).selectOption('setup');
  const themes = page.getByRole('radiogroup', { name: 'Theme of this screen' });
  await expect(themes.getByRole('radio', { name: 'LIGHT' })).toHaveAttribute('aria-checked', 'true');
  // Kept for this screen, and the menu agrees at desktop width.
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await page.setViewportSize({ width: 1600, height: 1000 });
  await switchTheme(page, 'dark');
  await page.getByRole('button', { name: 'Setup', exact: true }).click();
  await expect(themes.getByRole('radio', { name: 'DARK' })).toHaveAttribute('aria-checked', 'true');
  // The Setup view switches it too.
  await themes.getByRole('radio', { name: 'LIGHT' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
});

test('the header shows status left of a separator and controls right of it, each item one height, at every width', async ({ page }) => {
  await live(page);
  await expect(page.getByTestId('consoles').locator('[data-console]').first()).toBeAttached();
  const found: string[] = [];
  for (const [width, height] of [...SIZES, [1301, 900], [1440, 900], [1600, 1000]] as const) {
    await page.setViewportSize({ width, height });
    const at = `${width}x${height}`;
    const layout = await page.evaluate(() => {
      const box = (el: Element | null) => (el && el.getBoundingClientRect().width > 0 ? el.getBoundingClientRect() : null);
      const status = document.querySelector('header [role="group"][aria-label="Status"]')!;
      const controls = document.querySelector('header [role="group"][aria-label="Controls"]')!;
      const shown = (root: Element) =>
        [...root.querySelectorAll<HTMLElement>(':scope > *, :scope > * > *')]
          .filter((el) => el.matches('button, [data-testid="consoles"], .conn') && box(el) && getComputedStyle(el).visibility !== 'hidden')
          .map((el) => {
            const r = el.getBoundingClientRect();
            return { name: (el.getAttribute('aria-label') ?? el.textContent ?? '').trim().slice(0, 20), h: Math.round(r.height), w: Math.round(r.width), left: r.left, right: r.right };
          });
      const s = shown(status);
      const c = shown(controls);
      return {
        status: s,
        controls: c,
        statusRight: s.length > 0 ? Math.max(...s.map((i) => i.right)) : null,
        controlsLeft: c.length > 0 ? Math.min(...c.map((i) => i.left)) : null,
        sep: (() => {
          const sep = document.querySelector('header .sep');
          return sep && getComputedStyle(sep).visibility !== 'hidden' ? box(sep) : null;
        })(),
        consoles: box(document.querySelector('[data-testid="consoles"]')),
      };
    });
    const names = layout.controls.map((c) => c.name);
    // USERS opens the panel that lists the surfaces at every width, and the account pill is always there.
    if (!names.some((n) => /^Users \d+(,|$)/.test(n))) found.push(`${at}: no users key in ${names}`);
    const pill = layout.controls.find((c) => c.name.startsWith('Account'));
    if (!pill) found.push(`${at}: no account pill`);
    // One width for any initials or none: a session arriving moves nothing.
    else if (pill.w !== 32) found.push(`${at}: account pill ${pill.w} px wide`);
    // Controls are one height, and so is every status item shown.
    const heights = new Set([...layout.status, ...layout.controls].map((c) => c.h));
    if (heights.size !== 1) found.push(`${at}: heights ${JSON.stringify([...layout.status, ...layout.controls])}`);
    // The surfaces show from 1301 px, the separator with them, between status and controls.
    if ((layout.consoles !== null) !== width > 1300) found.push(`${at}: surfaces ${layout.consoles !== null}`);
    if ((layout.sep !== null) !== (layout.consoles !== null)) found.push(`${at}: separator ${layout.sep !== null}`);
    // The surfaces stand against the divider: the reserved slots, empty mostly, sit left of them.
    if (layout.sep && layout.consoles && layout.sep.left - layout.consoles.right > 12) found.push(`${at}: ${Math.round(layout.sep.left - layout.consoles.right)} px between the surfaces and the divider`);
    if (layout.sep && layout.statusRight !== null && layout.controlsLeft !== null && !(layout.statusRight <= layout.sep.left && layout.sep.right <= layout.controlsLeft)) {
      found.push(`${at}: separator not between status and controls`);
    }
  }
  expect(found).toEqual([]);
});

test('the users panel lists every control surface with its strips, where the header has no room for them', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await live(page);
  await expect(page.getByTestId('consoles')).toBeHidden();
  const key = page.getByRole('button', { name: /^Users \d+(,|$)/ });
  await key.click();
  await expect(key).toHaveAttribute('aria-expanded', 'true');
  const panel = page.getByRole('region', { name: 'Users' });
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId('console').first()).toContainText(/(MCU|OSC), (strips \d+-\d+|disconnected)/);
  await key.click();
  await expect(panel).toBeHidden();
  // The example shape's OSC tablet is disconnected: the key says so, by a dot and in its name.
  await expect(key).toHaveAccessibleName(/^Users \d+, 1 surface disconnected$/);
  await expect(page.getByTestId('users-down')).toBeVisible();
});

test('the surfaces read as a quiet field on the header in either theme, never the near-black well', async ({ page }) => {
  await live(page);
  const surfaces = page.getByTestId('consoles');
  await expect(surfaces).toBeVisible();
  for (const theme of ['dark', 'light'] as const) {
    if (theme === 'light') await switchTheme(page, 'light');
    const [bg, field, well] = await surfaces.evaluate((e) => {
      const probe = (v: string) => {
        const d = document.createElement('div');
        d.style.background = `var(${v})`;
        document.body.append(d);
        const c = getComputedStyle(d).backgroundColor;
        d.remove();
        return c;
      };
      return [getComputedStyle(e).backgroundColor, probe('--panel-2'), probe('--well')];
    });
    expect(bg, theme).toBe(field);
    expect(bg, theme).not.toBe(well);
  }
  await switchTheme(page, 'dark');
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
