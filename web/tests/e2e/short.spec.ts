// A phone in landscape (under 500 px of height): the faders are on
// screen without scrolling, the header is one compact row, and notices
// show in the header because the status bar row is gone.

import { expect, test } from '@playwright/test';
import { RawClient, live, resetMock } from './support';

test.beforeEach(async ({ request }) => resetMock(request));

test('puts the channel faders above the fold and the notices in the header', async ({ page, baseURL }) => {
  await live(page);
  const height = page.viewportSize()!.height;
  expect(height).toBeLessThan(500);
  const track = (await page.locator('[role="slider"][data-path="/channels/ch1/fader"]').boundingBox())!;
  expect(track.y + track.height).toBeLessThanOrEqual(height);
  expect(track.height, 'fader travel').toBeGreaterThanOrEqual(100);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeInViewport();
  await expect(page.getByRole('button', { name: 'Pres 1 PFL' })).toBeInViewport();
  await expect(page.locator('footer.bar')).toBeHidden();

  // A refused write still reaches the operator.
  const other = await RawClient.open(baseURL!);
  try {
    await other.command({ type: 'touch', path: '/channels/ch1/fader' });
    const { x, y } = { x: track.x + track.width / 2, y: track.y + track.height / 2 };
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x, y + 20);
    await page.mouse.up();
    const notice = page.getByTestId('notice');
    await expect(notice).toBeVisible();
    expect(await notice.evaluate((el) => el.closest('header') !== null)).toBe(true);
  } finally {
    other.close();
  }
});
