import { expect, test } from '@playwright/test';

test('serves over HTTPS and speaks wss to the mock', async ({ page }) => {
  await page.goto('/');
  expect(page.url()).toMatch(/^https:/);
  await expect(page.getByTestId('link')).toContainText('LIVE');
  await expect(page.locator('footer .right')).toContainText('wss://');
  const mute = page.getByRole('button', { name: 'Guest 1 mute' });
  const before = await mute.getAttribute('aria-pressed');
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', before === 'true' ? 'false' : 'true');
});
