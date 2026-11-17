import { resolve } from 'node:path';
import { devices, expect, test, type Browser, type Locator, type Page } from '@playwright/test';

const OUT = resolve(import.meta.dirname, '../../../docs/screenshots');

/** A screen of its own session, connected and with its standby up. */
async function screen(browser: Browser, options: Parameters<Browser['newContext']>[0] = {}): Promise<Page> {
  const page = await (await browser.newContext(options)).newPage();
  // Without an answer here the mock's dev controls stay unmounted, as against a real server.
  await page.route('**/__mock/replicas', (r) => r.fulfill({ status: 404 }));
  await page.goto('/');
  await expect(page.getByTestId('link')).toHaveText(/LIVE/);
  await page.waitForFunction(() => (window as unknown as { __faderz?: { standbyReplica?: unknown } }).__faderz?.standbyReplica);
  return page;
}

/** A clip around `boxes`, `pad` pixels wider on every side, within the viewport. */
async function around(page: Page, locators: Locator[], pad = 16) {
  const boxes = await Promise.all(locators.map(async (l) => (await l.boundingBox())!));
  const view = page.viewportSize()!;
  const x = Math.max(0, Math.min(...boxes.map((b) => b.x)) - pad);
  const y = Math.max(0, Math.min(...boxes.map((b) => b.y)) - pad);
  const right = Math.min(view.width, Math.max(...boxes.map((b) => b.x + b.width)) + pad);
  const bottom = Math.min(view.height, Math.max(...boxes.map((b) => b.y + b.height)) + pad);
  return { x, y, width: right - x, height: bottom - y };
}

const view = (page: Page, name: string) => page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name }).click();

test('README screenshots', async ({ browser }) => {
  const alex = await screen(browser, { viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 2 });
  const sam = await screen(browser);
  const robin = await screen(browser);
  // Sam works on the lead vocal, Robin holds the fader of Guest 2.
  await sam.locator('[data-channel=ch7] .head').click();
  const fader = robin.getByRole('slider', { name: 'Guest 2 fader' });
  const box = (await fader.boundingBox())!;
  await robin.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await robin.mouse.down();
  await expect(alex.getByTestId('holder-ch4')).toHaveText(/\S/);
  await alex.waitForTimeout(3000);
  await alex.screenshot({ path: `${OUT}/console.png` });

  await alex.locator('[data-channel=ch7] .head').click();
  await alex.waitForTimeout(1500);
  await alex.screenshot({ path: `${OUT}/channel-processing.png` });
  await alex.keyboard.press('Escape');

  const users = alex.getByRole('button', { name: /^Users/ });
  await users.click();
  await alex.waitForTimeout(500);
  await alex.screenshot({ path: `${OUT}/users.png`, clip: await around(alex, [users, alex.locator('#users-panel')]) });
  await users.click();

  const engine = alex.getByTestId('engine');
  await engine.click();
  await alex.waitForTimeout(500);
  await alex.screenshot({ path: `${OUT}/engine-counters.png`, clip: await around(alex, [engine, alex.locator('#engine-counters')]) });
  await engine.click();

  await view(alex, 'AUX / N-1');
  await alex.waitForTimeout(2000);
  await alex.screenshot({ path: `${OUT}/aux-mix-minus.png` });

  await view(alex, 'AFV');
  await alex.waitForTimeout(500);
  // Below the header, whose right-hand chips the table's width would cut, and every column of the table.
  const top = (await alex.locator('header.top').boundingBox())!;
  const table = (await alex.getByRole('table', { name: 'AFV bindings' }).boundingBox())!;
  const y = top.y + top.height;
  await alex.screenshot({ path: `${OUT}/afv-tally.png`, clip: { x: 0, y, width: table.x + table.width + 16, height: 720 - y } });

  await view(alex, 'SCENES');
  await alex.waitForTimeout(500);
  await alex.screenshot({ path: `${OUT}/scenes.png`, clip: { x: 0, y: 0, width: 1160, height: 560 } });

  const tablet = await screen(browser, { ...devices['iPad Pro 11 landscape'], deviceScaleFactor: 2 });
  await tablet.getByRole('button', { name: 'Switch to light theme' }).click();
  await tablet.waitForTimeout(3000);
  await tablet.screenshot({ path: `${OUT}/tablet-light.png` });

  await robin.mouse.up();
});
