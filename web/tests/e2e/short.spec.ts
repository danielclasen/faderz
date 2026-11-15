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

test('a desk locked in landscape shows its key in portrait and unlocks there', async ({ page }) => {
  await live(page);
  const lock = page.getByTestId('desk-lock');
  const cdp = await page.context().newCDPSession(page);
  const hold = async () => {
    const box = (await lock.boundingBox())!;
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }] });
    await page.waitForTimeout(800);
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  };
  await hold();
  await expect(lock).toHaveAttribute('aria-pressed', 'true');
  // Turned to portrait: a phone hides an unlit key, never a lit one, and
  // the header still fits the screen.
  await page.setViewportSize({ width: 390, height: 664 });
  await expect(lock).toBeVisible();
  await expect(lock).toHaveText('LOCKED');
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(page.viewportSize()!.width);
  await hold();
  await expect(lock).toHaveAttribute('aria-pressed', 'false');
  await expect(lock).toBeHidden();
});

test('a notice longer than the header slot widens nothing and moves no header item', async ({ page, baseURL }) => {
  await live(page);
  const width = page.viewportSize()!.width;
  const page_ = () => page.evaluate(() => ({ inner: innerWidth, scroll: document.documentElement.scrollWidth }));
  // Left edges of the header items right of the notice slot.
  const edges = () =>
    page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>('header .right > :not(.mid)')]
        .map((el) => el.getBoundingClientRect())
        .filter((r) => r.width > 0)
        .map((r) => Math.round(r.left)),
    );
  const before = await edges();
  expect(before.length).toBeGreaterThan(0);
  expect(await page_()).toEqual({ inner: width, scroll: width });
  const path = '/channels/ch4/fader';
  const other = await RawClient.open(baseURL!);
  try {
    expect((await other.command({ type: 'touch', path })).type).toBe('delta');
    // A keyboard adjust of the held fader is refused at once with a
    // notice wider than the slot.
    await page.locator(`[role="slider"][data-path="${path}"]`).focus();
    await page.keyboard.press('ArrowUp');
    const notice = page.getByTestId('notice');
    await expect(notice).toBeVisible();
    const after = await page_();
    expect(after.inner, 'layout viewport width').toBe(width);
    expect(after.scroll, 'page width').toBeLessThanOrEqual(after.inner);
    expect(await edges()).toEqual(before);
    // It is long: the slot cuts it off, and its whole text is in the tooltip.
    const text = notice.locator('.text');
    expect(await text.evaluate((el) => el.scrollWidth > el.clientWidth), 'the notice is cut off').toBe(true);
    expect(await notice.getAttribute('data-tip')).toBe(await text.textContent());
  } finally {
    await other.command({ type: 'release', path });
    other.close();
  }
});
