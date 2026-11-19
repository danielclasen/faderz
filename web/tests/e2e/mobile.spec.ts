import { expect, test } from '@playwright/test';
import { REPLICAS, debug, live, switchTheme, uiFontLoaded } from './support';

test('renders the mixer on a phone over a plain-HTTP LAN origin', async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  expect(baseURL).not.toContain('127.0.0.1');
  expect(await page.evaluate(() => window.isSecureContext)).toBe(false);
  expect(await page.evaluate(() => typeof (crypto as { randomUUID?: unknown }).randomUUID)).toBe('undefined');
  await expect(page.getByTestId('link')).toContainText('LIVE');
  // No horizontal page scroll: the header and the bank bar fit the phone.
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
  expect(await uiFontLoaded(page)).toBe(true);
  await expect(page.getByTestId('fatal')).toHaveCount(0);
  // Phone projects share one mock server, so toggle relative to what is there.
  const mute = page.getByRole('button', { name: 'Pres 1 mute' });
  const before = await mute.getAttribute('aria-pressed');
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');
  expect(errors).toEqual([]);
});

test('a phone header holds the view menu, the users key and the account pill, one finger tall, and no theme key', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('link')).toContainText('LIVE');
  const header = page.locator('header.top');
  await expect(header.getByRole('button', { name: /theme$/ })).toHaveCount(0);
  // The surfaces are read in the users panel here.
  await expect(page.getByTestId('consoles')).toBeHidden();
  const users = header.getByRole('button', { name: /^Users \d+$/ });
  const account = header.getByTestId('account');
  const W = page.viewportSize()!.width;
  for (const key of [users, account]) {
    const box = (await key.boundingBox())!;
    expect(box.height).toBe(32);
    expect(box.x + box.width).toBeLessThanOrEqual(W);
  }
  await expect(users).toHaveText(/^USR\s+\d+$/);
  await users.click();
  const panel = page.getByRole('region', { name: 'Users' });
  await expect(panel.getByTestId('console').first()).toBeVisible();
  const p = (await panel.boundingBox())!;
  expect(p.x).toBeGreaterThanOrEqual(0);
  expect(p.x + p.width).toBeLessThanOrEqual(W);
  await users.click();
  // The theme switches from the account menu.
  await switchTheme(page, 'light');
  await switchTheme(page, 'dark');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(W);
});

test('shows one duck group at a time on a phone, picked from a list, within the screen', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('link')).toContainText('LIVE');
  await page.getByRole('combobox', { name: 'View' }).selectOption({ label: 'Duck' });
  await expect(page.getByRole('switch', { name: 'Pres 1 triggers Commentary' })).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Phone 1 triggers Remote comm' })).toHaveCount(0);
  await page.getByRole('combobox', { name: 'Duck group' }).selectOption({ label: 'Remote comm' });
  await expect(page.getByRole('switch', { name: 'Phone 1 triggers Remote comm' })).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Pres 1 triggers Commentary' })).toHaveCount(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
  const table = (await page.getByRole('table', { name: 'Duck groups' }).boundingBox())!;
  expect(table.x + table.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  // The group says what keys it, what it ducks where, and its level against the threshold.
  await expect(page.getByTestId('duck-view-keys-remote')).toHaveText(/^key: Phone 1, Phone 2 \(/);
  await expect(page.getByTestId('duck-view-ducks-remote')).toHaveText(/^ducks: .* on PGM 5\.1, PGM 2\.0$/);
  await expect(page.getByTestId('duck-view-keying-remote')).toHaveText(/^level: .* dBFS$/);
});

test('keeps the commit banner readable on its own row when quorum is lost on a phone', { tag: '@mock' }, async ({ page, request }) => {
  test.setTimeout(60_000);
  await live(page);
  const primary = (await debug(page, 'primaryReplica'))!;
  for (const r of REPLICAS.filter((x) => x !== primary)) expect((await request.post(`/__mock/replicas/${r}/partition`)).ok()).toBe(true);
  try {
    const banner = page.getByTestId('commit-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('NOT COMMITTING');
    await expect(banner).toContainText('audio continues');
    const width = page.viewportSize()!.width;
    const box = (await banner.boundingBox())!;
    // The bar's padding and the dev mock key's reserve aside, it has the whole row.
    expect(box.width, 'the banner has the bar to itself').toBeGreaterThan(240);
    expect(await banner.locator('.text').evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
    // Above the keys of the bar, none of them cut off.
    const quorum = (await page.getByTestId('quorum').boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(quorum.y);
    expect(quorum.x + quorum.width).toBeLessThanOrEqual(width);
    // The short form says NO QUORUM and carries the whole text for touch and readers.
    await expect(page.getByTestId('quorum')).toHaveText('NO QUORUM');
    await expect(page.getByTestId('quorum')).toHaveAttribute('aria-label', /No quorum: 0 of 3 replicas can commit, 2 needed/);
    // The session is whole in the connection details.
    await page.getByTestId('quorum').click();
    await expect(page.getByTestId('session-detail')).not.toBeEmpty();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  } finally {
    for (const r of REPLICAS) await request.post(`/__mock/replicas/${r}/restore`);
  }
});

test('a tap on About / What\'s new in the connection details opens the changelog within a phone screen, and a tap on its close key closes it', async ({ page }) => {
  await live(page);
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).tap();
  const opener = page.getByTestId('build-about');
  // A finger-sized key.
  expect((await opener.boundingBox())!.height).toBeGreaterThanOrEqual(36);
  await opener.tap();
  const dialog = page.getByRole('dialog', { name: "About / What's new" });
  await expect(dialog.getByTestId('about-changelog')).toBeVisible();
  const v = page.viewportSize()!;
  const r = (await dialog.boundingBox())!;
  expect(r.x >= 0 && r.y >= 0 && r.x + r.width <= v.width && r.y + r.height <= v.height, JSON.stringify(r)).toBe(true);
  // Its body scrolls, not the page.
  expect(await dialog.locator('.body').evaluate((b) => b.scrollHeight > b.clientHeight)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(v.width);
  const close = dialog.getByTestId('about-close');
  expect((await close.boundingBox())!.width).toBeGreaterThanOrEqual(44);
  await close.tap();
  await expect(dialog).toBeHidden();
});

test('a tap outside About / What\'s new on a phone closes it', async ({ page }) => {
  await live(page);
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).tap();
  await page.getByTestId('build-about').tap();
  const dialog = page.getByRole('dialog', { name: "About / What's new" });
  await expect(dialog.getByTestId('about-changelog')).toBeVisible();
  // Below the dialog, over the backdrop.
  const r = (await dialog.boundingBox())!;
  const v = page.viewportSize()!;
  expect(r.y + r.height).toBeLessThan(v.height - 8);
  await page.touchscreen.tap(v.width / 2, (r.y + r.height + v.height) / 2);
  await expect(dialog).toBeHidden();
});
