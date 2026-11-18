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

test('shows the title of what a finger rests on, since touch has no hover', async ({ page }) => {
  await live(page);
  const cdp = await page.context().newCDPSession(page);
  const rest = async (box: { x: number; y: number; width: number; height: number }) => {
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }] });
    await page.waitForTimeout(800);
  };
  const info = page.getByTestId('touch-info');
  // A label with a tooltip of its own shows that one only: the strip's
  // source is in the tooltip of its head.
  await rest((await page.locator('[data-channel="ch2"] .head').boundingBox())!);
  await expect(page.getByRole('tooltip')).toHaveText('Pres 2 (Studio mic 2)');
  await expect(info).toHaveCount(0);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  const masters = page.getByRole('button', { name: 'MASTERS' });
  await rest((await masters.boundingBox())!);
  await expect(info).toBeVisible();
  await expect(info).toHaveText('Show the masters over the strips');
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  // The lift after a long touch reads the title; it does not press the key.
  await page.waitForTimeout(300);
  await expect(masters).toHaveAttribute('aria-pressed', 'false');
  // The next touch anywhere closes it.
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: 5, y: 5 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await expect(info).toHaveCount(0);
});

test('shows no title over a TALK key a finger holds, since that hold talks', async ({ page, request }) => {
  await live(page);
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
  const tab = page.getByRole('button', { name: 'Aux / N-1', exact: true });
  if (await tab.isVisible()) await tab.click();
  else await page.getByRole('combobox', { name: 'View' }).selectOption({ label: 'Aux / N-1' });
  const talk = page.getByRole('button', { name: /^N-1 Phone 1 talk/ });
  await talk.scrollIntoViewIfNeeded();
  const box = (await talk.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }] });
  await page.waitForTimeout(800);
  await expect(page.getByTestId('touch-info')).toHaveCount(0);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
});

test('operates a duck group from the MASTERS sheet without a scroll, a whole bank of strips under it', async ({ page, request }) => {
  await live(page);
  const width = page.viewportSize()!.width;
  const box = (await page.locator('.scroller').boundingBox())!;
  if (width >= 8 * STRIP_W + 20) expect(box.width).toBeGreaterThanOrEqual(8 * STRIP_W);
  await page.getByRole('button', { name: 'MASTERS' }).click();
  // The tiles come first in the sheet: every control of one is on screen as it opens.
  const view = page.viewportSize()!;
  const tile = page.locator('[data-duck="comm"]');
  for (const control of await tile.locator('button, [role="slider"]').all()) {
    const r = (await control.boundingBox())!;
    expect(r.x >= 0 && r.y >= 0 && r.x + r.width <= view.width && r.y + r.height <= view.height, `${await control.getAttribute('aria-label')} at ${JSON.stringify(r)}`).toBe(true);
  }
  const mode = async () => (await serverState(request)).state.ducks['comm']!.mode;
  try {
    // A tap latches it on and the next one sets it back.
    await page.getByTestId('duck-key-comm').tap();
    await expect.poll(mode).toBe('on');
    await page.getByTestId('duck-key-comm').tap();
    await expect.poll(mode).toBe('auto');
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
  }
});
