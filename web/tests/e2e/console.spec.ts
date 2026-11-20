// The header and status bar around the views: the connection details
// under their key, the quorum beside the link, the signed-in user's pill,
// the engine lamp and session at the bar's left end, and the boxes that
// must fit what they hold (LOCK DESK, the duck band, the EQ curve).

import { expect, test, type Page } from '@playwright/test';
import { debug, live, REPLICAS, resetMock } from './support';

test.beforeEach(async ({ request }) => resetMock(request));

const EMAIL = 'jane.doe@example.org';

/** The page signed in through OpenID Connect as EMAIL, an operator. */
async function signedIn(page: Page): Promise<string[]> {
  const logouts: string[] = [];
  await page.route('**/api/v1/session', (r) => r.fulfill({ json: { mode: 'oidc', user: EMAIL, role: 'operator' } }));
  await page.route('**/api/v1/logout', (r) => {
    logouts.push(r.request().method());
    return r.fulfill({ json: { redirect: null } });
  });
  return logouts;
}

const box = async (page: Page, selector: string) => (await page.locator(selector).first().boundingBox())!;

test('opens the connection details above their key in the status bar, inside the viewport, at every width', async ({ page }) => {
  await live(page);
  const key = page.getByRole('button', { name: /^Connection LIVE, details$/ });
  const details = page.getByRole('region', { name: 'Connection details' });
  for (const [width, height] of [[1600, 1000], [1194, 834], [820, 1180], [390, 844], [320, 568]] as const) {
    await page.setViewportSize({ width, height });
    const at = `${width}x${height}`;
    // In the bar, right of the engine lamp.
    const bar = (await box(page, 'footer.bar'))!;
    const k0 = (await key.boundingBox())!;
    const e0 = (await page.getByTestId('engine').boundingBox())!;
    expect(k0.y, `${at}: key in the bar`).toBeGreaterThanOrEqual(bar.y);
    expect(k0.y + k0.height, `${at}: key in the bar`).toBeLessThanOrEqual(bar.y + bar.height);
    expect(k0.x - (e0.x + e0.width), `${at}: right of the engine`).toBeGreaterThanOrEqual(0);
    expect(k0.x - (e0.x + e0.width), `${at}: right of the engine`).toBeLessThanOrEqual(14);
    expect(k0.x + k0.width, `${at}: whole in the bar`).toBeLessThanOrEqual(bar.x + bar.width);
    await key.click();
    await expect(details).toBeVisible();
    const k = (await key.boundingBox())!;
    const d = (await details.boundingBox())!;
    // Right above the key, not at the far edge of the screen.
    expect(d.y + d.height, `${at}: bottom`).toBeLessThanOrEqual(k.y);
    expect(d.y + d.height, `${at}: bottom`).toBeGreaterThanOrEqual(k.y - 8);
    expect(d.y, `${at}: top edge`).toBeGreaterThanOrEqual(0);
    const clamped = d.x <= 8.5 || d.x + d.width >= width - 8.5;
    expect(clamped || Math.abs(d.x - k.x) <= 1, `${at}: left ${d.x} over key at ${k.x}`).toBe(true);
    expect(d.x, `${at}: spans the key`).toBeLessThanOrEqual(k.x + k.width);
    expect(d.x + d.width, `${at}: spans the key`).toBeGreaterThanOrEqual(k.x);
    expect(d.x, `${at}: left edge`).toBeGreaterThanOrEqual(0);
    expect(d.x + d.width, `${at}: right edge`).toBeLessThanOrEqual(width);
    // Nothing inside runs out of the panel or over another cell.
    expect(await details.evaluate((e) => e.scrollWidth <= e.clientWidth && e.scrollHeight <= e.clientHeight), `${at}: overflow`).toBe(true);
    const rows = await details.locator('tr.replica').all();
    // A row per replica of the set the server reports: three on the mock,
    // one on a server run alone.
    const total = Number(/of (\d+) can commit/.exec((await page.getByTestId('quorum-detail').textContent()) ?? '')?.[1]);
    expect(total, `${at}: the quorum counts the set`).toBeGreaterThanOrEqual(1);
    expect(rows, `${at}: a row per replica`).toHaveLength(total);
    for (const row of rows) {
      const cells = await row.locator('td').evaluateAll((tds) => tds.map((td) => td.getBoundingClientRect()).map((r) => [r.left, r.right]));
      for (let i = 1; i < cells.length; i++) expect(cells[i]![0]!, `${at}: cell ${i} overlaps the one before`).toBeGreaterThanOrEqual(cells[i - 1]![1]! - 0.5);
    }
    await page.keyboard.press('Escape');
    await expect(details).toBeHidden();
    await expect(key).toBeFocused();
  }
});

test('the failover chip in the header covers no other header control', { tag: '@mock' }, async ({ page, request }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await live(page);
  const from = (await debug(page, 'primaryReplica'))!;
  expect((await request.post(`/__mock/replicas/${from}/kill`)).ok()).toBe(true);
  const chip = page.getByTestId('failover');
  await expect(chip).toBeVisible();
  const c = (await chip.boundingBox())!;
  // Every other control and readout of the header's right group.
  const others = await page.locator('header.top .right').evaluate((right) =>
    [...right.querySelectorAll<HTMLElement>('button, [data-testid]')]
      .filter((e) => e.dataset['testid'] !== 'failover' && e.dataset['testid'] !== 'status' && e.getBoundingClientRect().width > 0)
      .map((e) => ({ name: e.dataset['testid'] ?? e.textContent?.trim() ?? e.className, l: e.getBoundingClientRect().left, r: e.getBoundingClientRect().right })),
  );
  expect(others.length).toBeGreaterThan(3);
  for (const o of others) expect(c.x + c.width <= o.l + 0.5 || c.x >= o.r - 0.5, `chip ${c.x}-${c.x + c.width} over ${o.name} ${o.l}-${o.r}`).toBe(true);
  // Its text fits its slot, with its padding on both sides.
  const slot = (await chip.locator('..').boundingBox())!;
  expect(c.x + c.width).toBeLessThanOrEqual(slot.x + slot.width + 0.5);
  expect(await chip.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
});

test('the quorum sits in the link key, says what it counts, and follows a replica going down', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const quorum = page.getByTestId('quorum');
  await expect(quorum).toHaveText('QUORUM 3/3');
  await expect(quorum).toHaveAttribute('data-quorum', 'held');
  // Beside the link lamp, in one group, not a lone letter apart from it.
  const link = (await page.getByRole('button', { name: /^Connection LIVE, details$/ }).boundingBox())!;
  const q = (await quorum.boundingBox())!;
  expect(q.x - (link.x + link.width)).toBeLessThanOrEqual(2);
  expect(Math.abs(q.y - link.y)).toBeLessThanOrEqual(1);
  // The tooltip, by keyboard, names the counts, the majority and the leader.
  await quorum.focus();
  const tip = page.getByRole('tooltip');
  await expect(tip).toBeVisible();
  const leader = (await page.request.get('/__mock/replicas').then((r) => r.json()) as Array<{ id: string; role: string }>).find((r) => r.role === 'leader')!.id;
  await expect(tip).toContainText('Quorum held: 3 of 3 replicas can commit, 2 needed');
  await expect(tip).toContainText(`Leader ${leader}`);
  // A replica that neither serves nor stands by goes down: still a quorum, now amber.
  const primary = (await debug(page, 'primaryReplica'))!;
  const standby = (await debug(page, 'standbyReplica'))!;
  const third = REPLICAS.find((r) => r !== primary && r !== standby)!;
  expect((await request.post(`/__mock/replicas/${third}/kill`)).ok()).toBe(true);
  await expect(quorum).toHaveText('QUORUM 2/3');
  await expect(quorum).toHaveAttribute('data-quorum', 'degraded');
  await quorum.blur();
  await quorum.focus();
  await expect(tip).toContainText('2 of 3 replicas can commit');
  await expect(tip).toContainText(`Down: ${third}`);
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  await expect(page.getByTestId('quorum-detail')).toHaveText('held, 2 of 3 can commit, 2 needed');
  await expect(page.locator(`[data-replica="${third}"]`)).toHaveAttribute('data-healthy', 'false');
  // The width stays: nothing beside it moves.
  expect((await quorum.boundingBox())!.width).toBe(q.width);
  expect((await request.post(`/__mock/replicas/${third}/restore`)).ok()).toBe(true);
  await expect(quorum).toHaveText('QUORUM 3/3');
});

test('names the signed-in user by initials in a pill at the header end, with Sign out in its menu and no email anywhere', async ({ page, request }) => {
  const { build } = (await (await request.get('/api/v1/replicas')).json()) as { build: { faderz: string; samplez: string } };
  const logouts = await signedIn(page);
  await live(page);
  const pill = page.getByTestId('account');
  await expect(pill).toHaveText('JD');
  await expect(pill).toHaveAccessibleName('Account, operator');
  // The last item of the header, at its right end.
  const header = (await box(page, 'header.top'))!;
  const p = (await pill.boundingBox())!;
  expect(header.x + header.width - (p.x + p.width)).toBeLessThanOrEqual(12);
  expect(p.y).toBeGreaterThanOrEqual(header.y);
  expect(p.y + p.height).toBeLessThanOrEqual(header.y + header.height);
  // The status bar no longer names anybody or signs out.
  await expect(page.locator('footer.bar')).not.toContainText('operator');
  await expect(page.locator('footer.bar').getByRole('button', { name: 'Sign out' })).toHaveCount(0);
  // The menu opens under the pill, keyboard-operable.
  await pill.focus();
  await page.keyboard.press('ArrowUp');
  const menu = page.getByRole('menu', { name: 'Account' });
  await expect(menu).toBeVisible();
  // Settings first, then Sign out, then the version lines that open
  // About / What's new; ArrowUp from the pill lands on the last.
  await expect(menu.getByRole('menuitem')).toHaveText(['Switch to light theme', 'Sign out', `faderz ${build.faderz} samplez ${build.samplez}`]);
  await expect(page.getByTestId('account-build')).toBeFocused();
  await page.keyboard.press('ArrowUp');
  await expect(menu.getByRole('menuitem', { name: 'Sign out' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(menu.getByRole('menuitem', { name: 'Switch to light theme' })).toBeFocused();
  await expect(page.getByTestId('account-role')).toHaveText('operator');
  const m = (await menu.boundingBox())!;
  expect(m.y).toBeGreaterThanOrEqual(header.y + header.height);
  expect(m.y).toBeLessThanOrEqual(header.y + header.height + 8);
  expect(Math.abs(m.x + m.width - (p.x + p.width))).toBeLessThanOrEqual(1);
  expect(m.x + m.width).toBeLessThanOrEqual(1600);
  // Neither the address nor its local part is in the page, text or attributes.
  for (const part of [EMAIL, 'jane.doe', 'example.org']) expect(await page.content()).not.toContain(part);
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();
  await expect(pill).toBeFocused();
  // A press elsewhere closes it; Sign out signs out.
  await pill.click();
  await expect(menu).toBeVisible();
  await page.locator('main').click({ position: { x: 5, y: 5 } });
  await expect(menu).toBeHidden();
  // The theme entry switches the theme.
  await pill.click();
  await menu.getByRole('menuitem', { name: 'Switch to light theme' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await pill.click();
  await menu.getByRole('menuitem', { name: 'Switch to dark theme' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await pill.click();
  await menu.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect.poll(() => logouts).toEqual(['POST']);
});

test('names what the serving replica was built from in the connection details and under the account menu', async ({ page, request }) => {
  const { build } = (await (await request.get('/api/v1/replicas')).json()) as { build: { faderz: string; samplez: string } };
  expect(build.faderz).toMatch(/^\d+\.\d+\.\d+/);
  expect(build.samplez).toMatch(/^\d+\.\d+\.\d+/);
  await signedIn(page);
  await live(page);
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  await expect(page.getByTestId('build-faderz')).toHaveText(build.faderz);
  await expect(page.getByTestId('build-samplez')).toHaveText(build.samplez);
  await page.keyboard.press('Escape');
  await page.getByTestId('account').click();
  const menu = page.getByRole('menu', { name: 'Account' });
  await expect(menu.getByTestId('account-build')).toHaveText(`faderz ${build.faderz} samplez ${build.samplez}`);
  // Below the settings and Sign out, as the entry that opens About / What's new.
  await expect(menu.getByRole('menuitem').last()).toHaveAccessibleName(`About / What's new: faderz ${build.faderz}, samplez ${build.samplez}`);
  const signOut = (await menu.getByRole('menuitem', { name: 'Sign out' }).boundingBox())!;
  expect((await menu.getByTestId('account-build').boundingBox())!.y).toBeGreaterThanOrEqual(signOut.y + signOut.height);
});

test('the version lines open About / What\'s new with the changelog the replica serves, by keyboard and by pointer', async ({ page, request }) => {
  const about = (await (await request.get('/api/v1/about')).json()) as { build: { faderz: string; samplez: string }; changelog: string };
  // The newest release, as its heading reads.
  const newest = /^## \[([^\]]+)\]\([^)]*\)\s*(.*)$/m.exec(about.changelog)!;
  await signedIn(page);
  await live(page);
  // From the account menu by keyboard: the version entry is its last.
  await page.getByTestId('account').focus();
  await page.keyboard.press('ArrowUp');
  await expect(page.getByTestId('account-build')).toBeFocused();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog', { name: "About / What's new" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('about-build')).toHaveText(`faderz ${about.build.faderz}, built against samplez ${about.build.samplez}`);
  await expect(dialog.getByRole('heading', { level: 3 }).first()).toHaveText(`${newest[1]} ${newest[2]}`);
  await expect(dialog.getByRole('heading', { name: 'Changelog' })).toHaveCount(0);
  // Section labels read in the text colour at the list's size or as
  // capitals; breaking changes stand out from them.
  const label = dialog.getByRole('heading', { level: 4, name: 'Features' }).first();
  const item = dialog.locator('li').first();
  const style = (l: typeof label) => l.evaluate((e) => { const c = getComputedStyle(e); return { color: c.color, size: parseFloat(c.fontSize), transform: c.textTransform, weight: c.fontWeight }; });
  const [l, li] = [await style(label), await style(item)];
  expect(l.color).toBe(li.color);
  expect(l.size >= li.size || l.transform === 'uppercase').toBe(true);
  const breaking = dialog.getByRole('heading', { level: 4, name: /BREAKING CHANGES/ }).first();
  const b = await style(breaking);
  expect(b.color === l.color && b.weight === l.weight).toBe(false);
  // Every link opens a page of its own, none of them script.
  const hrefs = await dialog.getByTestId('about-changelog').locator('a').evaluateAll((as) => as.map((a) => [a.getAttribute('href'), a.getAttribute('target')]));
  expect(hrefs.length).toBeGreaterThan(10);
  expect(hrefs.filter(([href, target]) => !/^https:\/\//.test(href ?? '') || target !== '_blank')).toEqual([]);
  // Inside the viewport, its body scrolls.
  const r = (await dialog.boundingBox())!;
  const v = page.viewportSize()!;
  expect(r.x >= 0 && r.y >= 0 && r.x + r.width <= v.width && r.y + r.height <= v.height).toBe(true);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId('account')).toBeFocused();

  // From the connection details by pointer: closing goes back to them.
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  await page.getByTestId('build-about').click();
  await expect(dialog.getByTestId('about-changelog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('region', { name: 'Connection details' })).toBeVisible();
  await expect(page.getByTestId('build-about')).toBeFocused();
  // A press outside the dialog closes it too.
  await page.getByTestId('build-about').click();
  await expect(dialog).toBeVisible();
  await page.mouse.click(5, 5);
  await expect(dialog).toBeHidden();
});

test('About / What\'s new shows that it loads, and a failure with a way to try again', async ({ page }) => {
  let release: () => void = () => {};
  let fail = true;
  await page.route('**/api/v1/about', async (route) => {
    await new Promise<void>((r) => (release = r));
    if (fail) await route.fulfill({ status: 500, json: { code: 500, message: 'the replica broke' } });
    else await route.continue();
  });
  await live(page);
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  await page.getByTestId('build-about').click();
  const dialog = page.getByRole('dialog', { name: "About / What's new" });
  await expect(dialog.getByTestId('about-busy')).toHaveText('Loading the changelog...');
  await expect(dialog.locator('[aria-busy="true"]')).toHaveCount(1);
  release();
  await expect(dialog.getByRole('alert')).toContainText('The changelog could not be loaded: the replica broke');
  await expect(dialog.locator('[aria-busy="true"]')).toHaveCount(0);
  fail = false;
  await dialog.getByRole('button', { name: 'Try again' }).click();
  await expect(dialog.getByTestId('about-busy')).toBeVisible();
  release();
  await expect(dialog.getByTestId('about-changelog')).toBeVisible();
  await expect(dialog.getByRole('alert')).toHaveCount(0);
});

test('the pill fits the header from 320 px beside every other header control, a locked desk key included', async ({ page }) => {
  await signedIn(page);
  await live(page);
  const fits = async (at: string) => {
    for (const el of await page.locator('header.top').locator('button:visible, select:visible').all()) {
      const b = (await el.boundingBox())!;
      const name = `${at}: ${(await el.textContent())?.trim().slice(0, 12)}`;
      expect(b.x, name).toBeGreaterThanOrEqual(-0.5);
      expect(b.x + b.width, name).toBeLessThanOrEqual(page.viewportSize()!.width + 0.5);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), at).toBe(true);
  };
  const sizes = [[320, 568], [390, 844], [701, 900], [820, 1180], [1024, 768], [1600, 1000]] as const;
  for (const [width, height] of sizes) {
    await page.setViewportSize({ width, height });
    await expect(page.getByTestId('account')).toBeVisible();
    await fits(`${width}`);
  }
  // A locked desk shows its key as well, at every width.
  await page.setViewportSize({ width: 1600, height: 1000 });
  const lock = page.getByTestId('desk-lock');
  await lock.hover();
  await page.mouse.down();
  await expect(lock).toHaveAttribute('aria-pressed', 'true');
  await page.mouse.up();
  for (const [width, height] of sizes) {
    await page.setViewportSize({ width, height });
    await expect(lock).toBeVisible();
    await fits(`${width} locked`);
  }
  await lock.hover();
  await page.mouse.down();
  await expect(lock).toHaveAttribute('aria-pressed', 'false');
  await page.mouse.up();
});

test('puts the engine lamp at the status bar left end, the connection and the session in a row after it', async ({ page }) => {
  await live(page);
  for (const width of [1600, 820, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    const bar = (await box(page, 'footer.bar'))!;
    const engine = (await page.getByTestId('engine').boundingBox())!;
    const link = (await page.getByRole('button', { name: /^Connection LIVE, details$/ }).boundingBox())!;
    const quorum = (await page.getByTestId('quorum').boundingBox())!;
    const session = (await page.getByTestId('session').boundingBox())!;
    expect(engine.x - bar.x, `${width}: engine at the left`).toBeLessThanOrEqual(12);
    expect(link.x - (engine.x + engine.width), `${width}: link beside the engine`).toBeGreaterThanOrEqual(0);
    expect(link.x - (engine.x + engine.width), `${width}: link beside the engine`).toBeLessThanOrEqual(14);
    expect(quorum.x - (link.x + link.width), `${width}: quorum in the link key`).toBeLessThanOrEqual(2);
    expect(session.x - (quorum.x + quorum.width), `${width}: session after the quorum`).toBeGreaterThanOrEqual(0);
    expect(session.x - (quorum.x + quorum.width), `${width}: session after the quorum`).toBeLessThanOrEqual(14);
    const notices = (await box(page, 'footer.bar .notices'))!;
    expect(notices.x, `${width}: notices after the session`).toBeGreaterThanOrEqual(session.x + session.width);
    expect(quorum.x + quorum.width, `${width}: nothing cut off`).toBeLessThanOrEqual(bar.x + bar.width);
  }
});

test('the quorum turns red as NO QUORUM when two of three replicas are down', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const quorum = page.getByTestId('quorum');
  await expect(quorum).toHaveText('QUORUM 3/3');
  const primary = (await debug(page, 'primaryReplica'))!;
  for (const r of REPLICAS.filter((x) => x !== primary)) expect((await request.post(`/__mock/replicas/${r}/kill`)).ok()).toBe(true);
  await expect(quorum).toHaveText('NO QUORUM 0/3');
  await expect(quorum).toHaveAttribute('data-quorum', 'lost');
  expect(await quorum.evaluate((e) => getComputedStyle(e).backgroundColor)).toBe(await page.evaluate(() => {
    const probe = document.createElement('i');
    probe.style.color = 'var(--err)';
    document.body.append(probe);
    const c = getComputedStyle(probe).color;
    probe.remove();
    return c;
  }));
  await quorum.focus();
  await expect(page.getByRole('tooltip')).toContainText('No quorum: 0 of 3 replicas can commit, 2 needed');
  for (const r of REPLICAS) await request.post(`/__mock/replicas/${r}/restore`);
  await expect(quorum).toHaveText('QUORUM 3/3');
});

test('on a touch screen the link key, the quorum and the engine lamp have 44 px hit areas, and a tap on the quorum opens the details', async ({ browser, baseURL }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, baseURL });
  const page = await context.newPage();
  try {
    await live(page);
    expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
    for (const [name, locator] of [
      ['link key', page.getByRole('button', { name: /^Connection LIVE, details$/ })],
      ['quorum', page.getByTestId('quorum')],
      ['engine lamp', page.getByTestId('engine')],
    ] as const) {
      const r = (await locator.boundingBox())!;
      // The pointer finds the control 20 px above and below its centre and
      // 20 px to either side, whatever its drawn size.
      const hits = await locator.evaluate((el, [cx, cy]) =>
        [[0, -20], [0, 20], [-20, 0], [20, 0]].map(([dx, dy]) => el.contains(document.elementFromPoint(cx! + dx!, cy! + dy!))), [r.x + r.width / 2, r.y + r.height / 2]);
      expect(hits, `${name}: ${JSON.stringify(r)}`).toEqual([true, true, true, true]);
    }
    await page.getByTestId('quorum').tap();
    await expect(page.getByRole('region', { name: 'Connection details' })).toBeVisible();
    await expect(page.getByTestId('quorum-detail')).toHaveText(/^held, (\d+) of \1 can commit, \d+ needed$/);
  } finally {
    await context.close();
  }
});

test('LOCK DESK keeps its padding on both sides of its text', async ({ page }) => {
  await live(page);
  const lock = page.getByTestId('desk-lock');
  for (const width of [1600, 1100]) {
    await page.setViewportSize({ width, height: 900 });
    const gaps = await lock.evaluate((b) => {
      const label = b.querySelector('.label')!;
      const range = document.createRange();
      range.selectNodeContents(label);
      const text = range.getBoundingClientRect();
      const key = b.getBoundingClientRect();
      return { left: text.left - key.left, right: key.right - text.right, label: label.scrollWidth - label.clientWidth };
    });
    expect(gaps.label, `${width}: text fits its label`).toBeLessThanOrEqual(0);
    expect(gaps.left, `${width}: left padding`).toBeGreaterThanOrEqual(5);
    expect(gaps.right, `${width}: right padding`).toBeGreaterThanOrEqual(5);
    expect(Math.abs(gaps.left - gaps.right), `${width}: centred`).toBeLessThanOrEqual(2);
  }
});

test('the duck band in the masters shows its tiles without a scrollbar', async ({ page }) => {
  for (const [width, height] of [[1600, 1000], [1920, 1080], [1400, 900]] as const) {
    await page.setViewportSize({ width, height });
    await live(page);
    const band = page.getByRole('group', { name: 'Duck groups' });
    await expect(band).toBeVisible();
    const sizes = await band.evaluate((e) => ({ scroll: e.scrollHeight, client: e.clientHeight }));
    expect(sizes.scroll, `${width}x${height}`).toBeLessThanOrEqual(sizes.client);
  }
});

test('the EQ curve in the processing panel is as wide as the panel gives it', async ({ page }) => {
  for (const [width, height] of [[1600, 1000], [820, 1180]] as const) {
    await page.setViewportSize({ width, height });
    await live(page);
    await page.locator('[data-channel="ch1"] .head').click();
    const curve = page.locator('svg.curve.fill');
    await expect(curve).toBeVisible();
    const sizes = await curve.evaluate((svg) => {
      const parent = svg.parentElement!;
      const style = getComputedStyle(parent);
      const inner = parent.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      return { curve: svg.getBoundingClientRect().width, inner };
    });
    expect(sizes.curve, `${width}x${height}`).toBeGreaterThan(300);
    expect(Math.abs(sizes.curve - sizes.inner), `${width}x${height}`).toBeLessThanOrEqual(1);
    await page.keyboard.press('Escape');
  }
});

test('a replica serving a newer faderz version lights an UPDATE key, moving and covering nothing; its panel opens on a press, LATER clears the dot until the next one', { tag: '@mock' }, async ({ page, request }) => {
  for (const width of [1440, 1200, 1024, 960, 700]) {
    await page.setViewportSize({ width, height: 900 });
    await live(page);
    const key = page.getByTestId('update-key');
    const dot = page.getByTestId('update-dot');
    const panel = page.getByTestId('new-version');
    await expect(key).toBeHidden();
    const announce = page.getByTestId('update-announce');
    await expect(announce).toHaveCount(1);
    await expect(announce).toHaveText('');
    const fader = page.locator('[data-path$="/fader"]').first();
    const mute = page.locator('[data-path$="/mute"]').first();
    const at = async () => [(await fader.boundingBox())!, (await mute.boundingBox())!, (await key.boundingBox())!].map((b) => [b.x, b.y]);
    const before = await at();
    const session = await debug(page, 'session');
    const roll = async (faderz: string) => expect((await request.post(`/__mock/build?session=${session}&faderz=${faderz}`)).ok()).toBe(true);
    await roll(`99.0.${width}`);
    await expect(key).toBeVisible();
    await expect(dot).toBeVisible();
    // Nothing moved and nothing opened on its own; the header holds all
    // of its keys whole.
    expect(await at(), `${width} px`).toEqual(before);
    await expect(panel).toBeHidden();
    const header = page.locator('header.top');
    expect(await header.evaluate((h) => h.scrollWidth <= h.clientWidth), `${width} px header overflows`).toBe(true);
    for (const k of [key, page.getByRole('button', { name: /^Users/ }), page.getByTestId('desk-lock')]) {
      expect(await k.evaluate((b) => b.scrollWidth <= b.clientWidth && b.scrollHeight <= b.clientHeight + 1), `${width} px key clipped`).toBe(true);
    }
    await expect(page.getByTestId('update-announce')).toHaveText(`A new faderz version is running (99.0.${width})`);
    // Escape and a press outside close it.
    await key.click();
    await expect(panel).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(panel).toBeHidden();
    await expect(key).toBeFocused();
    await key.click();
    await page.mouse.click(5, 300);
    await expect(panel).toBeHidden();
    await key.click();
    await expect(panel).toContainText(`A new faderz version is running (99.0.${width})`);
    await panel.getByRole('button', { name: 'LATER' }).click();
    await expect(panel).toBeHidden();
    await expect(dot).toBeHidden();
    await expect(key).toBeVisible();
    await roll(`99.1.${width}`);
    await expect(dot).toBeVisible();
    // An older one is no news.
    await roll('0.0.1');
    await expect(key).toBeHidden();
    await roll(`99.2.${width}`);
    await key.click();
    await Promise.all([page.waitForEvent('load'), panel.getByRole('button', { name: 'RELOAD' }).click()]);
    expect(await page.evaluate(() => (performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming).type)).toBe('reload');
    await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  }
});

test('below 430 px the account pill stands in for the UPDATE key: a dot, and the update in its menu, moving nothing', { tag: '@mock' }, async ({ page, request }) => {
  await page.setViewportSize({ width: 320, height: 700 });
  await live(page);
  const dot = page.getByTestId('account-update-dot');
  const pill = page.getByTestId('account');
  await expect(page.getByTestId('update-key')).toBeHidden();
  await expect(dot).toBeHidden();
  const announce = page.getByTestId('update-announce');
  await expect(announce).toHaveCount(1);
  await expect(announce).toHaveText('');
  const before = (await pill.boundingBox())!;
  const session = await debug(page, 'session');
  expect((await request.post(`/__mock/build?session=${session}&faderz=99.0.0`)).ok()).toBe(true);
  await expect(dot).toBeVisible();
  await expect(announce).toHaveText('A new faderz version is running (99.0.0)');
  expect((await pill.boundingBox())!).toEqual(before);
  await pill.click();
  await page.getByRole('menuitem', { name: 'Update later' }).click();
  await expect(dot).toBeHidden();
  await pill.click();
  await expect(page.getByRole('menuitem', { name: 'Reload to update to 99.0.0' })).toBeVisible();
});
