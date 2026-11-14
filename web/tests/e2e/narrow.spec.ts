// Screens narrower than the strips and masters side by side (tablets,
// phones in landscape): the strip area shows whole strips, at least a
// bank of 8 where the width allows, and the masters lie over the strips
// only while MASTERS is lit.

import { expect, test } from '@playwright/test';
import { live, patch, resetMock, serverState } from './support';

const STRIP_W = 86;

test.beforeEach(async ({ request }) => resetMock(request));

test('shows whole strips, a whole bank, and the masters over the strips on request', async ({ page }) => {
  await live(page);
  const width = page.viewportSize()!.width;
  const scroller = page.locator('.scroller');
  const box = (await scroller.boundingBox())!;
  expect(box.width % STRIP_W, `scroller ${box.width} px`).toBe(0);
  if (width >= 8 * STRIP_W + 20) expect(box.width).toBeGreaterThanOrEqual(8 * STRIP_W);

  // Every strip in view is whole, also after a scroll that stops mid-strip.
  const cut = () =>
    page.evaluate(() => {
      const s = document.querySelector('.scroller')!.getBoundingClientRect();
      return [...document.querySelectorAll<HTMLElement>('.strip')]
        .map((el) => el.getBoundingClientRect())
        .filter((r) => r.right > s.left + 1 && r.left < s.right - 1)
        .filter((r) => r.left < s.left - 1 || r.right > s.right + 1).length;
    });
  expect(await cut()).toBe(0);
  await scroller.evaluate((el) => el.scrollBy({ left: 130 }));
  await page.waitForTimeout(600);
  expect(await cut()).toBe(0);
  await page.getByRole('button', { name: '1-8' }).click();
  await page.waitForTimeout(600);

  // The masters are an overlay: nothing under them moves when they open.
  const pgm = page.locator('[data-master="pgm51"]');
  const masters = page.getByRole('button', { name: 'MASTERS' });
  await expect(pgm).toBeHidden();
  const strip = page.locator('[data-channel="ch1"]');
  const before = await strip.boundingBox();
  await masters.click();
  await expect(masters).toHaveAttribute('aria-pressed', 'true');
  await expect(pgm).toBeVisible();
  await expect(page.getByRole('slider', { name: 'PGM 5.1 fader' })).toBeVisible();
  expect(await strip.boundingBox()).toEqual(before);
  // The bank bar stays clear of the overlay, so MASTERS closes it again.
  await masters.click();
  await expect(pgm).toBeHidden();
});

test('gives touch screens targets a finger can hit on the controls used most', async ({ page }) => {
  await live(page);
  expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
  const h = async (loc: ReturnType<typeof page.locator>) => (await loc.boundingBox())!.height;
  expect(await h(page.getByRole('button', { name: 'Pres 1 PFL' }))).toBeGreaterThanOrEqual(36);
  expect(await h(page.getByRole('button', { name: 'Pres 1 AFL' }))).toBeGreaterThanOrEqual(36);
  expect(await h(page.getByRole('button', { name: 'Pres 1 mute' }))).toBeGreaterThanOrEqual(32);
  expect(await h(page.getByRole('button', { name: '1-8' }))).toBeGreaterThanOrEqual(32);
  expect(await h(page.getByRole('slider', { name: 'Pres 1 pan' }))).toBeGreaterThanOrEqual(28);
});

test('moves a strip pan on touch only after the finger rests on it', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch1/pan/lr', 0);
  const pan = page.getByRole('slider', { name: 'Pres 1 pan' });
  await expect(pan).toHaveAttribute('aria-valuenow', '0');
  const box = (await pan.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const touch = (type: 'touchStart' | 'touchMove' | 'touchEnd', x: number) =>
    cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y: box.y + box.height / 2 }] });
  const x0 = box.x + box.width / 4;
  // A finger brushing across on its way to PFL: nothing moves.
  await touch('touchStart', x0);
  for (let i = 1; i <= 4; i++) await touch('touchMove', x0 + i * 8);
  await touch('touchEnd', 0);
  await page.waitForTimeout(400);
  expect((await serverState(request)).state.channels['ch1']!.pan.lr).toBe(0);
  // Resting first, then moving: the pan follows.
  await touch('touchStart', x0);
  await page.waitForTimeout(450);
  for (let i = 1; i <= 4; i++) await touch('touchMove', x0 + i * 8);
  await touch('touchEnd', 0);
  await expect.poll(async () => (await serverState(request)).state.channels['ch1']!.pan.lr).toBeGreaterThan(0);
});
