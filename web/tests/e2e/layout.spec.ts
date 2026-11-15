// Owner rule: text length changes, live updates and interactions never
// move layout. Every layout shift the Layout Instability API reports
// counts here, including those right after input that CLS itself would
// excuse, and the total must be 0.

import { expect, test, type Page } from '@playwright/test';
import { HEADERS, RawClient, debug, dragFader, live, patch, resetMock } from './support';

interface Shift {
  value: number;
  input: boolean;
  sources: string[];
}

declare global {
  interface Window {
    __shifts?: Shift[];
    __facesAtMount?: string[];
  }
}

test.beforeEach(async ({ page, request }) => {
  await resetMock(request);
  await page.addInitScript(() => {
    const shifts: Shift[] = [];
    window.__shifts = shifts;
    const describe = (node: Node | null) => {
      const n = node instanceof Element ? node : (node?.parentElement ?? null);
      if (!n) return String(node?.nodeName ?? 'null');
      const cls = typeof n.className === 'string' && n.className ? `.${n.className.trim().split(/\s+/).join('.')}` : '';
      const data = [...n.attributes].filter((a) => a.name.startsWith('data-') || a.name === 'aria-label').map((a) => `[${a.name}=${a.value}]`).join('');
      return `${n.tagName.toLowerCase()}${cls}${data}"${(n.textContent ?? '').trim().slice(0, 16)}"`;
    };
    new PerformanceObserver((list) => {
      type Rect = { x: number; y: number; width: number; height: number };
      const r = (a: Rect) => `${Math.round(a.x)},${Math.round(a.y)} ${Math.round(a.width)}x${Math.round(a.height)}`;
      for (const e of list.getEntries() as unknown as Array<{ value: number; hadRecentInput: boolean; sources: Array<{ node: Node | null; previousRect: Rect; currentRect: Rect }> }>) {
        // The dev mock's control panel is not part of the product.
        const product = e.sources.filter((s) => !(s.node instanceof Element && s.node.closest('aside.mock')) && !s.node?.parentElement?.closest('aside.mock'));
        if (product.length === 0) continue;
        shifts.push({ value: e.value, input: e.hadRecentInput, sources: product.map((s) => `${describe(s.node)} ${r(s.previousRect)} -> ${r(s.currentRect)}`) });
      }
    }).observe({ type: 'layout-shift', buffered: true });
  });
});
test.afterEach(async ({ request }) => resetMock(request));

/** Opens a view by its tab, or by the view menu where tabs are folded (phones). */
async function showView(page: Page, name: string): Promise<void> {
  const tab = page.getByRole('button', { name, exact: true });
  if (await tab.isVisible()) await tab.click();
  else await page.getByRole('combobox', { name: 'View' }).selectOption({ label: name });
}

async function shifts(page: Page): Promise<Shift[]> {
  // Let the observer deliver the entries of the last frames.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  return page.evaluate(() => window.__shifts ?? []);
}

async function expectNoShift(page: Page, step: string): Promise<void> {
  const list = await shifts(page);
  expect(list, `${step}: ${JSON.stringify(list.slice(0, 6))}`).toEqual([]);
}

test('mounts only once the web fonts are in, however late they arrive, so no text moves', async ({ page }) => {
  // Later than the app waited before it registered faces itself.
  await page.route(/\.woff2(\?|$)/, async (route) => {
    await new Promise((r) => setTimeout(r, 3000));
    await route.continue();
  });
  await page.addInitScript(() => {
    new MutationObserver((_, observer) => {
      if (!document.getElementById('app')?.firstElementChild) return;
      observer.disconnect();
      window.__facesAtMount = [...document.fonts].filter((f) => f.status === 'loaded').map((f) => `${f.family.replace(/["']/g, '')} ${f.weight}`);
    }).observe(document, { childList: true, subtree: true });
  });
  await page.goto('/');
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live', { timeout: 15_000 });
  expect(await page.evaluate(() => window.__facesAtMount)).toEqual(
    expect.arrayContaining(['IBM Plex Sans Condensed 400', 'IBM Plex Sans Condensed 500', 'IBM Plex Sans Condensed 600', 'JetBrains Mono 400', 'JetBrains Mono 600']),
  );
  await expectNoShift(page, 'load with late fonts');
});

test('nothing moves on load, live updates, failover and interaction', async ({ page, request }, info) => {
  test.setTimeout(60_000);
  const phone = (page.viewportSize()?.width ?? 1600) < 600;
  await live(page);
  await page.evaluate(() => document.fonts.ready);
  await expect(page.locator('[data-testid^="loudness-i-"]').first()).not.toHaveText('--.-', { timeout: 10_000 });
  await expectNoShift(page, 'load');

  // Live state updates from other clients and the switcher.
  for (const [path, value] of [
    ['/channels/ch3/fader', -33.3],
    ['/channels/ch3/fader', 0],
    ['/channels/ch4/mute', true],
    ['/channels/ch4/mute', false],
    ['/channels/ch1/input', 'b'],
    ['/channels/ch1/input', 'a'],
    ['/channels/ch3/pan/lr', -0.6],
    ['/channels/ch3/pan/lr', 0.85],
    ['/channels/ch3/pan/lr', 0],
    ['/vcas/vca2/offset', -12.5],
    ['/vcas/vca2/offset', -2],
    // ch3 in a second VCA, then back: the foot row keeps its height.
    ['/vcas/vca1/members', ['ch1', 'ch2', 'ch3']],
    ['/vcas/vca1/offset', -6],
    ['/vcas/vca1/offset', 0],
    ['/vcas/vca1/members', ['ch1', 'ch2']],
    ['/tone/targets', ['pgm20']],
    ['/tone/on', true],
    ['/tone/on', false],
    ['/tone/targets', []],
    ['/metering/loudness/tolerance', 3],
    ['/metering/scale', 'ebu18'],
    ['/metering/scale', 'ebu9'],
    ['/metering/loudness/tolerance', 1],
    ['/channels/ch2/meterTap', 'input'],
    ['/channels/ch2/meterTap', 'preFader'],
  ] as const) await patch(request, path, value);
  await request.put('/api/v1/afv/tally', { headers: { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' }, data: { sources: { CAM1: 'off', CAM2: 'program' } } });
  await page.waitForTimeout(1500);
  await expectNoShift(page, 'live updates');

  // Failover.
  const primary = await debug(page, 'primaryReplica');
  await request.post(`/__mock/replicas/${primary}/kill`);
  await expect.poll(() => debug(page, 'failovers')).toBeGreaterThan(0);
  await expect.poll(() => debug(page, 'connection')).toBe('Live');
  await page.waitForTimeout(500);
  await request.post(`/__mock/replicas/${primary}/restart`);
  await expectNoShift(page, 'failover');

  // A dropped link: the primary endpoint empties while reconnecting.
  await request.post('/__mock/drop-clients');
  await expect.poll(() => debug(page, 'connection')).not.toBe('Live');
  await expect.poll(() => debug(page, 'connection'), { timeout: 10_000 }).toBe('Live');
  await page.waitForTimeout(300);
  await expectNoShift(page, 'reconnect');

  // Interaction on the mixer.
  await dragFader(page, '/channels/ch2/fader', -40);
  await page.getByRole('button', { name: 'Pres 2 mute' }).click();
  await page.getByRole('button', { name: 'Pres 2 PFL' }).click();
  await page.getByRole('button', { name: 'Pres 2 PFL' }).click();
  await page.getByRole('button', { name: 'Pres 2 mute' }).click();
  await expectNoShift(page, 'fader, mute, PFL');

  const bank = page.getByRole('button', { name: '9-16' });
  if (await bank.isVisible()) {
    await bank.click();
    await page.waitForTimeout(600);
    await page.getByRole('button', { name: '1-8' }).click();
    await page.waitForTimeout(600);
    await expectNoShift(page, 'bank change');
  }

  const sof = page.getByRole('combobox', { name: 'Sends on faders' });
  if (await sof.isVisible()) {
    await sof.selectOption({ label: 'IFB 1' });
    await page.waitForTimeout(300);
    await page.getByTestId('sends-on-faders').click();
    await expectNoShift(page, 'sends on faders');
  }

  const masters = page.getByRole('button', { name: 'MASTERS' });
  if (await masters.isVisible()) {
    await masters.click();
    await page.waitForTimeout(300);
    await masters.click();
    await expectNoShift(page, 'masters over the strips');
  }

  await page.locator('[data-channel="ch1"] .head').click();
  await page.waitForTimeout(300);
  await expectNoShift(page, 'open processing');
  await page.keyboard.press('Escape');
  await expectNoShift(page, 'close processing');

  const scene = `Layout ${Date.now()}`;
  const created = await request.post('/api/v1/scenes', { headers: HEADERS, data: { commandId: crypto.randomUUID(), name: scene } });
  expect(created.ok()).toBe(true);
  for (const name of ['Routing', 'Aux / N-1', 'AFV', 'Scenes', 'Setup', 'Mixer']) {
    await showView(page, name);
    await page.waitForTimeout(400);
    await expectNoShift(page, `view ${name}`);
    if (name === 'Scenes') {
      // Arming STORE or DEL puts CONFIRM in its place, and disarming takes it back.
      for (const key of [`Store into ${scene}`, `Delete ${scene}`]) {
        await page.getByRole('button', { name: key, exact: true }).click();
        await page.waitForTimeout(200);
        await expectNoShift(page, `arm ${key}`);
        await page.keyboard.press('Escape');
        await page.waitForTimeout(200);
        await expectNoShift(page, `disarm ${key}`);
      }
    }
    if (name === 'AFV') {
      // Tally cuts while the AFV table is in view.
      for (const sources of [
        { CAM1: 'off', CAM2: 'off', CAM3: 'program', CAM4: 'preview' },
        { CAM1: 'off', CAM2: 'off', CAM3: 'off', CAM4: 'off' },
        { CAM1: 'program', CAM2: 'preview', CAM3: 'off', CAM4: 'off' },
      ]) {
        await request.put('/api/v1/afv/tally', { headers: { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' }, data: { sources } });
      }
      await patch(request, '/afv/ch13/enabled', false);
      await patch(request, '/afv/ch13/source', null);
      await patch(request, '/afv/ch13/source', 'CAM1');
      await patch(request, '/afv/ch13/enabled', true);
      await page.waitForTimeout(600);
      await expectNoShift(page, 'tally on the AFV view');
    }
  }
  const doc = (await (await request.get('/api/v1/state')).json()) as { state: { scenes: Array<{ id: string; name: string }> } };
  const id = doc.state.scenes.find((x) => x.name === scene)!.id;
  expect((await request.delete(`/api/v1/scenes/${id}`, { headers: HEADERS })).ok()).toBe(true);
  info.annotations.push({ type: 'viewport', description: phone ? 'phone' : 'desktop' });
});

test('every fader fits its box, VCA masters included', async ({ page }) => {
  await live(page);
  // Below 1400 px the masters lie over the strips while MASTERS is lit.
  const masters = page.getByRole('button', { name: 'MASTERS' });
  if (await masters.isVisible()) await masters.click();
  const over = await page.locator('.fader').evaluateAll((list) =>
    list.flatMap((f) => {
      const box = f.parentElement!.getBoundingClientRect();
      const r = f.getBoundingClientRect();
      const w = [...f.children].reduce((a, c) => a + c.getBoundingClientRect().width, 0) + 2 * (f.children.length - 1);
      return w > box.width + 0.5 || r.right > box.right + 0.5 ? [`${f.closest('section')?.getAttribute('aria-label')}: ${w} > ${box.width}`] : [];
    }),
  );
  expect(over).toEqual([]);
});

test('no visible label is cut off without its full text in a tooltip reachable by keyboard', async ({ page, request, baseURL }) => {
  // A long channel name (ch8), a long VCA name (vca2), a listen list
  // longer than the monitor's box and a fader another session holds.
  const other = await RawClient.open(baseURL!);
  expect((await other.command({ type: 'touch', path: '/channels/ch4/fader' })).type).toBe('delta');
  for (const ch of ['ch1', 'ch2', 'ch3', 'ch4', 'ch5', 'ch6', 'ch8']) await patch(request, `/channels/${ch}/solo`, 'pfl');
  await patch(request, '/channels/ch7/solo', 'afl');
  await patch(request, '/vcas/vca2/members', ['ch3', 'ch4', 'ch5', 'ch6', 'ch8']);
  await live(page);
  await expect(page.getByTestId('holder-ch4')).not.toHaveText('');
  await page.evaluate(() => document.fonts.ready);
  const views = ['Mixer', 'Routing', 'Aux / N-1', 'AFV', 'Scenes', 'Setup'];
  const problems: string[] = [];
  for (const name of views) {
    await showView(page, name);
    await page.waitForTimeout(300);
    if (name === 'Mixer') {
      await page.locator('[data-channel="ch1"] .head').click();
      await page.waitForTimeout(200);
    }
    problems.push(
      ...(await page.evaluate((view) => {
        const out: string[] = [];
        for (const el of document.querySelectorAll<HTMLElement>('body *')) {
          if (el.children.length > 0 && [...el.childNodes].every((n) => n.nodeType !== Node.TEXT_NODE || !n.textContent?.trim())) continue;
          const text = el.textContent?.trim() ?? '';
          if (!text) continue;
          const r = el.getBoundingClientRect();
          if (r.width === 0 || r.height === 0 || r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) continue;
          const cs = getComputedStyle(el);
          if (cs.visibility === 'hidden' || el.closest('.sr-only')) continue;
          const clipX = cs.overflowX === 'hidden' || cs.overflowX === 'clip' || cs.textOverflow === 'ellipsis';
          const clipY = cs.overflowY === 'hidden' || cs.overflowY === 'clip';
          const own = (clipX && el.scrollWidth > el.clientWidth + 1) || (clipY && el.scrollHeight > el.clientHeight + 1);
          // Cut by an ancestor that clips without scrolling, up to the
          // nearest one that scrolls (whose content is reached by scrolling).
          // The text's own extent, not the box's: a heading's box may be
          // wider than its words.
          const range = document.createRange();
          range.selectNodeContents(el);
          const t = range.getBoundingClientRect();
          let byParent = false;
          for (let p = el.parentElement; p && p !== document.body && !byParent; p = p.parentElement) {
            const ps = getComputedStyle(p);
            if ([ps.overflowX, ps.overflowY].some((o) => o === 'auto' || o === 'scroll')) break;
            const pr = p.getBoundingClientRect();
            if ((ps.overflowX === 'hidden' || ps.overflowX === 'clip') && (t.left < pr.left - 1 || t.right > pr.right + 1)) byParent = true;
            if ((ps.overflowY === 'hidden' || ps.overflowY === 'clip') && (t.top < pr.top - 1 || t.bottom > pr.bottom + 1)) byParent = true;
          }
          if (!own && !byParent) continue;
          // lib/tip.ts: shown on hover, keyboard focus and a long touch.
          const holder = el.closest<HTMLElement>('[data-tip]');
          const tipped = holder !== null && (holder.dataset['tip'] ?? '').includes(text) && holder.tabIndex >= 0;
          if (!tipped) out.push(`${view}: "${text}" (${el.tagName.toLowerCase()}.${el.className})`);
        }
        return out;
      }, name)),
    );
  }
  for (const ch of ['ch1', 'ch2', 'ch3', 'ch4', 'ch5', 'ch6', 'ch7', 'ch8']) await patch(request, `/channels/${ch}/solo`, 'off');
  await patch(request, '/vcas/vca2/members', ['ch3', 'ch4', 'ch5', 'ch6']);
  await other.command({ type: 'release', path: '/channels/ch4/fader' });
  other.close();
  expect(problems).toEqual([]);
});

test('a cut-off label shows its full text on hover, on keyboard focus and on a long touch', async ({ page }, info) => {
  await live(page);
  const tooltip = page.getByRole('tooltip');
  const head = page.locator('[data-channel="ch1"] .head');
  if (info.project.name === 'desktop') {
    await page.locator('[data-channel="ch8"] .head').hover();
    await expect(tooltip).toHaveText('Reporter 2 North Gate (Studio mic 8)');
    await page.mouse.move(0, 0);
    await expect(tooltip).toBeHidden();
    // Keyboard focus, which a mouse click does not give.
    await page.locator('[data-channel="ch8"] .head').focus();
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Tab');
    await expect(tooltip).toHaveText('Reporter 2 North Gate (Studio mic 8)');
    await expect(page.locator('[data-channel="ch8"] .head')).toHaveAttribute('aria-describedby', 'faderz-tip');
    await page.keyboard.press('Escape');
    await expect(tooltip).toBeHidden();
    return;
  }
  // A held touch shows the tooltip and does not also open the channel.
  const box = (await head.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const point = [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }];
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: point });
  await page.waitForTimeout(800);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await expect(tooltip).toHaveText('Pres 1 (Studio mic 1)');
  await expect(head).toHaveAttribute('aria-pressed', 'false');
  await expect(tooltip).toBeHidden({ timeout: 5000 });
});

test('no text an operator reads is set below 10 px', async ({ page }) => {
  await live(page);
  const small: string[] = [];
  for (const name of ['Mixer', 'Routing', 'Aux / N-1', 'AFV', 'Scenes', 'Setup']) {
    await showView(page, name);
    await page.waitForTimeout(300);
    if (name === 'Mixer') await page.locator('[data-channel="ch1"] .head').click();
    small.push(
      ...(await page.evaluate((view) => {
        const out: string[] = [];
        for (const el of document.querySelectorAll<HTMLElement>('body *')) {
          if (el.closest('.sr-only, aside.mock')) continue;
          const own = [...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && n.textContent?.trim());
          if (!own) continue;
          const r = el.getBoundingClientRect();
          const cs = getComputedStyle(el);
          if (r.width === 0 || r.height === 0 || cs.visibility === 'hidden') continue;
          const px = parseFloat(cs.fontSize);
          // 0 px is the brand folded away on phones: not shown at all.
          if (px > 0 && px < 10) out.push(`${view}: "${el.textContent?.trim().slice(0, 20)}" ${px}px (${el.tagName.toLowerCase()}.${el.className})`);
        }
        return out;
      }, name)),
    );
  }
  expect([...new Set(small)]).toEqual([]);
});

test('the dev mock key covers no control and no readout', async ({ page }) => {
  await live(page);
  const hits = await page.evaluate(() => {
    const key = document.querySelector('aside.mock button')!.getBoundingClientRect();
    const overlap = (r: DOMRect) => r.width > 0 && r.left < key.right && r.right > key.left && r.top < key.bottom && r.bottom > key.top;
    return [...document.querySelectorAll<HTMLElement>('button, [role="slider"], output, .fixed, .notice-line, [data-testid="session"]')]
      .filter((el) => !el.closest('aside.mock') && getComputedStyle(el).visibility !== 'hidden' && overlap(el.getBoundingClientRect()))
      .map((el) => `${el.tagName.toLowerCase()}.${el.className} "${el.textContent?.trim().slice(0, 16)}"`);
  });
  expect(hits).toEqual([]);
});

test('on a touch screen no control of the mixer is under 24 px (WCAG 2.5.8)', async ({ page }) => {
  await live(page);
  // The masters too, where they are an overlay.
  const masters = page.getByRole('button', { name: 'MASTERS' });
  if (await masters.isVisible()) await masters.click();
  const coarse = await page.evaluate(() => matchMedia('(pointer: coarse)').matches);
  const small = await page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>('button, [role="slider"], input, select, a[href]')]
      .filter((el) => !el.closest('aside.mock, .sr-only, .skip'))
      .filter((el) => {
        const r = el.getBoundingClientRect();
        const cs = getComputedStyle(el);
        return r.width > 0 && r.height > 0 && r.bottom > 0 && r.top < innerHeight && r.right > 0 && r.left < innerWidth && cs.visibility !== 'hidden' && cs.pointerEvents !== 'none';
      })
      .filter((el) => {
        const r = el.getBoundingClientRect();
        return Math.min(r.width, r.height) < 24;
      })
      .slice(0, 40)
      .map((el) => `${el.tagName.toLowerCase()}.${el.className} "${(el.getAttribute('aria-label') ?? el.textContent ?? '').trim().slice(0, 20)}" ${Math.round(el.getBoundingClientRect().width)}x${Math.round(el.getBoundingClientRect().height)}`),
  );
  expect(coarse ? [...new Set(small)] : []).toEqual([]);
});
