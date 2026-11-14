import { expect, test } from '@playwright/test';
import { debug, fader, live, resetMock, settled } from './support';

// Tracing is off: its DOM snapshot after every action runs on the throttled main thread.
test.use({ trace: 'off' });

test.beforeEach(async ({ request }) => resetMock(request));
test.afterEach(async ({ request }) => resetMock(request));

test('keeps the link live under 4x CPU throttling without a false failover', async ({ page, request }) => {
  test.setTimeout(60_000);
  await live(page);
  const cdp = await page.context().newCDPSession(page);
  const failovers = await debug(page, 'failovers');
  const primary = await debug(page, 'primaryReplica');
  await page.evaluate(() => {
    const link = document.querySelector('[data-testid="link"]')!;
    const seen = ((window as unknown as { __linkStates: string[] }).__linkStates = [link.getAttribute('data-state')!]);
    new MutationObserver(() => seen.push(link.getAttribute('data-state')!)).observe(link, { attributes: true, attributeFilter: ['data-state'] });
  });
  const path = '/channels/ch15/fader';
  const slider = page.locator(`[role="slider"][data-path="${path}"]`);
  await slider.focus();
  await page.keyboard.press('Alt+0');
  await settled(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: 4 });
  try {
    // About 4 s of writes, one step every 100 ms.
    for (let i = 0; i < 40; i++) {
      await page.keyboard.press('Shift+ArrowUp');
      await page.waitForTimeout(100);
    }
    await expect(slider).toHaveAttribute('aria-valuenow', '4');
    await settled(page);
  } finally {
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: 1 });
  }
  expect(await fader(request, 'ch15')).toBe(4);
  expect(await debug(page, 'failovers')).toBe(failovers);
  expect(await debug(page, 'primaryReplica')).toBe(primary);
  const seen = await page.evaluate(() => (window as unknown as { __linkStates: string[] }).__linkStates);
  expect(new Set(seen)).toEqual(new Set(['Live']));
});
