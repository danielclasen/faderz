import { expect, test } from '@playwright/test';

test('renders the mixer on a phone over a plain-HTTP LAN origin', async ({ page, baseURL }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  expect(baseURL).not.toContain('127.0.0.1');
  expect(await page.evaluate(() => window.isSecureContext)).toBe(false);
  expect(await page.evaluate(() => typeof (crypto as { randomUUID?: unknown }).randomUUID)).toBe('undefined');
  await expect(page.getByTestId('link')).toContainText('LIVE');
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
  await expect(page.getByTestId('fatal')).toHaveCount(0);
  // Phone projects share one mock server, so toggle relative to what is there.
  const mute = page.getByRole('button', { name: 'Pres 1 mute' });
  const before = await mute.getAttribute('aria-pressed');
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');
  expect(errors).toEqual([]);
});
