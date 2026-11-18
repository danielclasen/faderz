import { expect, test } from '@playwright/test';
import { uiFontLoaded } from './support';

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
});
