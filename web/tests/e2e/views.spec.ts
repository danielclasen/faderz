import { expect, test } from '@playwright/test';
import { live, resetMock } from './support';

test.beforeEach(async ({ request }) => resetMock(request));

test('renders every view without an error, with live meters on the aux masters', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await live(page);
  for (const name of ['Routing', 'Aux / N-1', 'AFV', 'Scenes', 'Setup', 'Mixer']) {
    await page.getByRole('button', { name, exact: true }).click();
    await expect(page.getByRole('button', { name, exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByTestId('view-error')).toHaveCount(0);
    if (name === 'Aux / N-1') {
      await expect(page.locator('[data-master="aux1"]')).toBeVisible();
      await expect(page.locator('[data-master="mm1"]')).toBeVisible();
    }
  }
  expect(errors).toEqual([]);
});
