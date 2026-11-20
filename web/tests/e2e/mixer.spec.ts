import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { handleTag, ownerHandle } from '../../src/lib/net/handle';
import { COUNTER_NAMES, counterInfo, engineLamp } from '../../src/lib/counters';
import { CONSOLE_SESSIONS } from '../../dev/mock/shape';
import {
  BACKEND,
  REPLICAS,
  RawClient,
  audit,
  commands,
  debug,
  dragFader,
  fader,
  faderCenter,
  formatDb,
  live,
  patch,
  resetMock,
  serverState,
  settled,
  switchTheme,
  uiFontLoaded,
} from './support';

/** RESET takes a one second hold. */
async function holdReset(page: Page, master: string): Promise<void> {
  const key = page.locator(`[data-master="${master}"]`).getByRole('button', { name: 'Reset loudness, hold for one second' });
  const box = (await key.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(1100);
  await page.mouse.up();
}

test.beforeEach(async ({ request }) => resetMock(request));
test.afterEach(async ({ request }) => resetMock(request));

test('loads the mixer from a snapshot and streams meters', { tag: '@meters' }, async ({ page }) => {
  await live(page);
  await expect(page.locator('[data-channel]')).toHaveCount(29);
  await expect(page.locator('[data-master]')).toHaveCount(4);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
  expect(await uiFontLoaded(page)).toBe(true);
  const primary = await debug(page, 'primaryReplica');
  await expect(page.getByTestId('meter-replica').locator('b')).toHaveText(primary!);
  await expect(page.getByTestId('loudness-i-pgm20')).not.toHaveText('--.-', { timeout: 10_000 });
  await expect(page.locator(`[data-replica="${primary}"]`)).toHaveAttribute('data-role', 'primary');
  await expect(page.locator('[data-role="standby"]')).toHaveCount(1);
  await expect(page.getByTestId('quorum')).toHaveAttribute('data-commitable', 'true');
});

test('moves a fader and shows the value the server echoed', async ({ page, request }) => {
  await live(page);
  const before = await fader(request, 'ch3');
  await dragFader(page, '/channels/ch3/fader', -60);
  await settled(page);
  const after = await fader(request, 'ch3');
  expect(after).toBeGreaterThan(before);
  await expect(page.getByTestId('fader-value-ch3')).toHaveText(formatDb(after));
  await expect.poll(async () => (await serverState(request)).owners['/channels/ch3/fader']).toBeUndefined();
});

test('gives each meter a level for assistive technology and shows the channel meter tap', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  // Post-fader by default: the meter moves with the fader.
  const meter = page.getByRole('meter', { name: /^Pres 1 meter, post-fader/ });
  await expect(meter).toHaveAttribute('aria-valuetext', /^peak -?\d+ dBFS/, { timeout: 5000 });
  await expect(page.getByTestId('tap-ch1')).toHaveText('POST');
  await patch(request, '/channels/ch1/meterTap', 'preFader');
  await expect(page.getByTestId('tap-ch1')).toHaveText(' PRE');
  await patch(request, '/channels/ch1/meterTap', 'postFader');
});

test('switches the channel meter between post- and pre-fader on the strip, without the processing panel', async ({ page, request }) => {
  await live(page);
  const key = page.getByRole('button', { name: /^Pres 1 meter tap, post-fader/ });
  await expect(key).toBeVisible();
  await key.click();
  await settled(page);
  expect((await serverState(request)).state.channels['ch1']!.meterTap).toBe('preFader');
  await expect(page.getByTestId('tap-ch1')).toHaveText(' PRE');
  await expect(page.getByRole('meter', { name: /^Pres 1 meter, pre-fader/ })).toBeAttached();
  await expect(page.getByRole('region', { name: 'Pres 1 processing' })).toHaveCount(0);
  await page.getByRole('button', { name: /^Pres 1 meter tap, pre-fader/ }).click();
  await settled(page);
  expect((await serverState(request)).state.channels['ch1']!.meterTap).toBe('postFader');
});

for (const vp of [{ width: 1600, height: 1000 }, { width: 390, height: 844 }]) {
  test(`the masters scroll only as far as their controls reach at ${vp.width}x${vp.height}`, async ({ page }) => {
    await page.setViewportSize(vp);
    await live(page);
    // Below 1400 px the masters are an overlay behind the MASTERS key.
    if (vp.width < 1400) await page.getByRole('button', { name: 'MASTERS' }).click();
    const masters = page.getByTestId('masters');
    await expect(masters).toBeVisible();
    // A closed fader moves its cap box down by the whole travel; that box
    // must add no empty scroll room below or beside the controls.
    const m = await masters.evaluate((el) => {
      const row = el.querySelector<HTMLElement>('.row')!.getBoundingClientRect();
      return { sw: el.scrollWidth, cw: el.clientWidth, sh: el.scrollHeight, ch: el.clientHeight, rw: Math.ceil(row.width), rh: Math.ceil(row.height) };
    });
    expect(m.sw).toBeLessThanOrEqual(Math.max(m.cw, m.rw));
    expect(m.sh).toBeLessThanOrEqual(Math.max(m.ch, m.rh));
    // Where the controls fit the pane, it does not scroll at all.
    if (m.rw <= m.cw) expect(m.sw).toBeLessThanOrEqual(m.cw);
    if (m.rh <= m.ch) expect(m.sh).toBeLessThanOrEqual(m.ch);
  });
}

test("a meter's level for assistive technology ends where its bars do when the meters stop", { tag: '@mock' }, async ({ page, request }) => {
  test.setTimeout(60_000);
  await live(page);
  const meters = page.locator('[role="meter"][data-meter^="channels/"]');
  await expect(meters.first()).toHaveAttribute('aria-valuetext', /^peak -?\d+ dBFS/, { timeout: 5000 });
  for (const r of REPLICAS) expect((await request.post(`/__mock/replicas/${r}/kill`)).ok()).toBe(true);
  // Bars and holds fall to the floor within about 8 s, then nothing draws.
  await page.waitForTimeout(10_000);
  const texts = await meters.evaluateAll((list) => list.filter((m) => m.getBoundingClientRect().width > 0).map((m) => m.getAttribute('aria-valuetext')));
  expect(texts.length).toBeGreaterThan(0);
  expect(texts.filter((t) => t !== 'no signal')).toEqual([]);
});

test('keeps the fader cap under the pointer while dragging', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch3/fader', -20);
  const cap = page.locator('[data-path="/channels/ch3/fader"] .cap');
  await expect(page.getByTestId('fader-value-ch3')).toHaveText(formatDb(-20));
  const before = (await cap.boundingBox())!.y;
  await dragFader(page, '/channels/ch3/fader', -60);
  await settled(page);
  await expect.poll(async () => Math.abs((await cap.boundingBox())!.y - before + 60)).toBeLessThan(2);
});

test("applies another client's change as a delta", async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch17/fader', -12.5);
  await expect(page.getByTestId('fader-value-ch17')).toHaveText('-12.5');
});

test('mutes a channel', async ({ page, request }) => {
  await live(page);
  const mute = page.getByRole('button', { name: 'Pres 2 mute' });
  const before = (await serverState(request)).state.channels['ch2']!.mute;
  await expect(mute).toHaveAttribute('aria-pressed', String(before));
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', String(!before));
  await settled(page);
  expect((await serverState(request)).state.channels['ch2']!.mute).toBe(!before);
});

test('stores and recalls a scene', async ({ page, request }) => {
  await live(page);
  const name = `E2E scene ${Date.now()}`;
  await page.getByRole('button', { name: 'Scenes' }).click();
  await page.getByLabel('New scene name').fill(name);
  await page.getByRole('button', { name: 'STORE NEW' }).click();
  await expect(page.getByText(name)).toBeVisible();
  const stored = await fader(request, 'ch18');
  await patch(request, '/channels/ch18/fader', stored === -33 ? -34 : -33);
  // Picking a scene recalls nothing; GO does, and every client sees it as current.
  await page.getByRole('button', { name: `Pick ${name} for recall` }).click();
  await page.waitForTimeout(300);
  expect(await fader(request, 'ch18')).not.toBe(stored);
  await page.getByRole('button', { name: 'Recall the picked scene' }).click();
  await expect.poll(() => fader(request, 'ch18')).toBe(stored);
  const id = (await serverState(request)).state.scenes.find((s) => s.name === name)!.id;
  await expect.poll(async () => (await serverState(request)).state.sceneRecalled?.id).toBe(id);
  await expect(page.locator(`[data-scene="${id}"] .tag`)).toBeVisible();
  // Storing over a scene asks for a confirmation in place.
  await page.getByRole('button', { name: `Store into ${name}` }).click();
  const confirmStore = page.getByRole('button', { name: `Confirm store into ${name}` });
  await expect(confirmStore).toBeVisible();
  // An armed confirmation does not stay armed: Escape, a press elsewhere and time disarm it.
  await page.keyboard.press('Escape');
  await expect(confirmStore).toBeHidden();
  await page.getByRole('button', { name: `Delete ${name}` }).click();
  const confirmDelete = page.getByRole('button', { name: `Confirm delete of ${name}` });
  await expect(confirmDelete).toBeVisible();
  await page.getByLabel('New scene name').click();
  await expect(confirmDelete).toBeHidden();
  await page.getByRole('button', { name: `Store into ${name}` }).click();
  await expect(confirmStore).toBeVisible();
  await expect(confirmStore).toBeHidden({ timeout: 6000 });
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.getByTestId('fader-value-ch18')).toHaveText(formatDb(stored));
});

test('arms and confirms a scene store and delete from the keyboard alone, within the arm time', async ({ page }) => {
  await live(page);
  const name = `E2E keys ${Date.now()}`;
  await page.getByRole('button', { name: 'Scenes' }).click();
  await page.getByLabel('New scene name').fill(name);
  await page.getByRole('button', { name: 'STORE NEW' }).click();
  const store = page.getByRole('button', { name: `Store into ${name}`, exact: true });
  const del = page.getByRole('button', { name: `Delete ${name}`, exact: true });
  await expect(store).toBeVisible();
  // Arming hands the focus to CONFIRM, and disarming hands it back.
  await store.focus();
  await page.keyboard.press('Enter');
  const confirmStore = page.getByRole('button', { name: `Confirm store into ${name}` });
  await expect(confirmStore).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(store).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(confirmStore).toBeFocused();
  // The arm time runs out: the focus is back on STORE, not on the page.
  await expect(confirmStore).toBeHidden({ timeout: 6000 });
  await expect(store).toBeFocused();
  // CONFIRM fits its column: nothing spills or moves when it arms.
  await del.focus();
  const before = await del.boundingBox();
  await page.keyboard.press(' ');
  const confirmDelete = page.getByRole('button', { name: `Confirm delete of ${name}` });
  await expect(confirmDelete).toBeFocused();
  const fit = await confirmDelete.evaluate((b) => {
    const li = b.closest('li')!.getBoundingClientRect();
    const r = b.getBoundingClientRect();
    return { spill: b.scrollWidth - b.clientWidth, past: r.right - li.right, w: r.width };
  });
  expect(fit.spill).toBeLessThanOrEqual(0);
  expect(fit.past).toBeLessThanOrEqual(0);
  expect(fit.w).toBeCloseTo(before!.width, 0);
  await page.keyboard.press('Enter');
  await expect(page.getByText(name)).toBeHidden();
});

test('asks the holder of a refused control for it, and tells this screen when another asks for one it holds', async ({ page, baseURL }) => {
  await live(page);
  const path = '/channels/ch4/fader';
  const other = await RawClient.open(baseURL!);
  try {
    expect((await other.command({ type: 'touch', path })).type).toBe('delta');
    const tag = handleTag(ownerHandle(other.session));
    await dragFader(page, path, -80);
    const notice = page.getByTestId('notice');
    await expect(notice).toContainText(`Guest 2 fader is held by web client ${tag}`);
    const handle = ownerHandle(await debug(page, 'session'));
    await notice.getByRole('button', { name: 'ASK' }).click();
    await expect.poll(() => other.messages.filter((m) => m.type === 'controlRequest')).toEqual([
      expect.objectContaining({ path, from: expect.objectContaining({ handle, kind: 'ui' }) }),
    ]);
    await expect(notice).toContainText(`Asked web client ${tag} for Guest 2 fader`);
    // Asking again at once sends nothing.
    await dragFader(page, path, -80);
    await notice.getByRole('button', { name: 'ASK' }).click();
    await page.waitForTimeout(500);
    expect(other.messages.filter((m) => m.type === 'controlRequest')).toHaveLength(1);
    // Released, the notice offers nothing to ask for.
    await other.command({ type: 'release', path });
    await expect(notice.getByRole('button', { name: 'ASK' })).toHaveCount(0);

    // This screen holds a fader while the other session asks for it.
    const mine = '/channels/ch6/fader';
    const at = await faderCenter(page, mine);
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await expect.poll(async () => (await debug(page, 'touching')) ?? []).toContain(mine);
    await expect.poll(async () => {
      other.send({ type: 'request', path: mine });
      return (await notice.textContent()) ?? '';
    }).toContain(`web client ${tag} asks for Guest 4 fader`);
    await page.mouse.up();
  } finally {
    other.close();
  }
});

test('marks the strip of a channel another screen has selected with its initial', async ({ page, baseURL }) => {
  await live(page);
  const other = await RawClient.open(baseURL!);
  try {
    const tag = handleTag(ownerHandle(other.session));
    const mark = page.getByTestId('selected-by-ch5');
    await expect(mark).toBeHidden();
    const head = page.locator('[data-channel="ch5"] .head');
    const boxes = () => head.locator('.name, .badge.shown, .no').evaluateAll((els) => els.map((e) => JSON.stringify(e.getBoundingClientRect())));
    const before = await boxes();
    other.send({ type: 'select', element: 'ch5' });
    await expect(mark).toBeVisible();
    await expect(mark).toHaveText(tag.charAt(0));
    // The mark takes room from the source line only: nothing else moves.
    expect((await boxes()).filter((b) => before.includes(b))).toHaveLength(before.length);
    await expect(head).toHaveAccessibleName(new RegExp(`selected on web client ${tag}`));
    // What this screen selects, the other session is told.
    const handle = ownerHandle(await debug(page, 'session'));
    await page.locator('[data-channel="ch2"] .head').click();
    await expect
      .poll(() => other.messages.filter((m) => m.type === 'selections').at(-1)?.['selections'])
      .toContainEqual(expect.objectContaining({ element: 'ch2', session: expect.objectContaining({ handle }) }));
    // This screen's own selection marks nothing here.
    await expect(page.getByTestId('selected-by-ch2')).toBeHidden();
    other.send({ type: 'select', element: null });
    await expect(mark).toBeHidden();
  } finally {
    other.close();
  }
});

test('refuses a fader held by another session at the gesture and shows its value and owner', { tag: '@mock' }, async ({ page, baseURL, request }) => {
  const frames: string[] = [];
  page.on('websocket', (ws) => ws.on('framereceived', (f) => typeof f.payload === 'string' && frames.push(f.payload)));
  await live(page);
  const path = '/channels/ch4/fader';
  const other = await RawClient.open(baseURL!);
  try {
    expect((await other.command({ type: 'touch', path })).type).toBe('delta');
    const owner = (await serverState(request)).owners[path];
    const handle = ownerHandle(other.session);
    expect(owner).toEqual({ principal: 'anonymous', handle, kind: 'ui', connection: expect.any(String) });
    await expect(page.locator('[data-channel="ch4"]').getByTestId('owner')).toHaveAttribute('data-owner-kind', 'ui');
    const held = await fader(request, 'ch4');
    const tag = handleTag(handle);
    await dragFader(page, path, -80);
    const notice = page.getByTestId('notice');
    await expect(notice).toContainText(`Guest 2 fader is held by web client ${tag}`);
    await dragFader(page, path, 40);
    await expect(notice).toHaveCount(1);
    await expect(notice.locator('.count')).toBeVisible();
    // The held fader refuses the gesture itself: no write left this client,
    // and the strip names the holder in words.
    expect((await commands(request)).filter((c) => c.path === path && c.type === 'set')).toEqual([]);
    // The strip shows the tag in full; the label is in the tooltip.
    const holder = page.getByTestId('holder-ch4');
    await expect(holder).toHaveText(tag);
    expect(await holder.locator('span').evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
    await expect(holder).toHaveAccessibleName(`Fader held by web client ${tag}`);
    // The keyboard reaches the full label on the fader, not on an extra stop.
    await expect(holder).toHaveJSProperty('tabIndex', -1);
    const track = page.locator('[role="slider"][data-path="/channels/ch4/fader"]');
    await track.focus();
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Tab');
    await expect(track).toBeFocused();
    await expect(page.getByRole('tooltip')).toHaveText(`Held by web client ${tag}`);
    await expect(track).toHaveAttribute('aria-valuetext', new RegExp(`held by web client ${tag}$`));
    // The notice sits in the status bar, clear of the monitor and masters.
    const box = (await notice.boundingBox())!;
    const monitor = (await page.getByRole('region', { name: 'Monitor' }).boundingBox())!;
    expect(box.y).toBeGreaterThanOrEqual(monitor.y + monitor.height);
    await settled(page);
    await expect(page.getByTestId('fader-value-ch4')).toHaveText(formatDb(held));
    expect(await fader(request, 'ch4')).toBe(held);
    expect((await other.command({ type: 'release', path })).type).toBe('delta');
    // The page names the holder by its handle and never sees its session id.
    expect(frames.some((f) => f.includes(handle))).toBe(true);
    expect(frames.filter((f) => f.includes(other.session))).toEqual([]);
    // Its own status bar shows the tag other clients see for it.
    await expect(page.getByTestId('session')).toContainText(handleTag(ownerHandle(await debug(page, 'session'))));
  } finally {
    other.close();
  }
});

test('reconnects and resyncs after a dropped link and after a revision gap', async ({ page, request }) => {
  await live(page);
  await request.post('/__mock/drop-clients');
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  await request.post('/__mock/skip-revision');
  const value = (await fader(request, 'ch19')) === -7 ? -8 : -7;
  await patch(request, '/channels/ch19/fader', value);
  await expect(page.getByTestId('fader-value-ch19')).toHaveText(formatDb(value));
  const server = await serverState(request);
  await expect(page.getByTestId('revision')).toHaveText(`rev ${server.revision}`);
});

test('moves nothing on its own when the system asks for reduced motion', async ({ page, request }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await live(page);
  await page.getByRole('button', { name: '9-16' }).click();
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(r)));
  expect(await page.locator('.scroller').evaluate((e) => e.scrollLeft)).toBe(8 * 86);
  await patch(request, '/tone/targets', ['me']);
  await patch(request, '/tone/on', true);
  await expect(page.getByTestId('tone')).toBeVisible();
  await expect.poll(() => page.getByTestId('tone').evaluate((e) => e.getAnimations().length)).toBe(0);
  await patch(request, '/tone/on', false);
  await patch(request, '/tone/targets', []);
});

test('switches to the light theme and works at tablet width', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await live(page);
  await switchTheme(page, 'light');
  const width = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(width).toBeLessThanOrEqual(1024);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
  await page.getByRole('button', { name: 'MASTERS' }).click();
  await expect(page.locator('[data-master="pgm51"]')).toBeVisible();
});

test('operates a fader from the keyboard', async ({ page, request }) => {
  await live(page);
  const slider = page.locator('[role="slider"][data-path="/channels/ch21/fader"]');
  await slider.focus();
  await page.keyboard.press('Alt+0');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Shift+ArrowDown');
  await settled(page);
  await expect(slider).toHaveAttribute('aria-valuenow', '0.4');
  expect(await fader(request, 'ch21')).toBe(0.4);
  // Neither a lone key nor a double-click jumps a fader.
  await page.keyboard.press('0');
  await page.keyboard.press('Home');
  await page.keyboard.press('End');
  const closed = page.locator('[role="slider"][data-path="/channels/ch22/fader"]');
  await closed.dblclick();
  await page.waitForTimeout(300);
  await settled(page);
  expect(await fader(request, 'ch21')).toBe(0.4);
  expect(await fader(request, 'ch22')).toBe(-90);
});

test('a fader dragged after a keyboard adjust keeps its touch when focus leaves it', async ({ page, request }) => {
  await live(page);
  const path = '/channels/ch21/fader';
  const owner = async () => (await serverState(request)).owners[path];
  const slider = page.locator(`[role="slider"][data-path="${path}"]`);
  await slider.focus();
  await page.keyboard.press('ArrowDown');
  await expect.poll(owner).toBeDefined();
  // The keyboard touch ends on its own after a pause.
  await expect.poll(owner, { timeout: 3000 }).toBeUndefined();
  const { x, y } = await faderCenter(page, path);
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y + 10);
  await expect.poll(owner).toBeDefined();
  // Focus leaves while the pointer still holds the fader.
  await page.keyboard.press('Tab');
  await page.waitForTimeout(500);
  expect(await owner()).toBeDefined();
  await page.mouse.up();
  await expect.poll(owner).toBeUndefined();
});

test('operates a pan from the keyboard, with no jump from a lone key or a double-click', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const pan = page.locator('[role="slider"][data-path="/channels/ch21/pan/lr"]');
  await pan.focus();
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Shift+ArrowLeft');
  await settled(page);
  await expect(pan).toHaveAttribute('aria-valuenow', '-0.06');
  // A key write holds the control like a fader key does, then lets go.
  await expect
    .poll(async () => (await audit(request)).entries.filter((e) => e.path === '/channels/ch21/pan/lr').map((e) => e.type))
    .toEqual(['touch', 'set', 'set', 'release']);
  await page.keyboard.press('0');
  await page.keyboard.press('Home');
  await page.keyboard.press('End');
  await pan.dblclick();
  await page.waitForTimeout(300);
  await settled(page);
  expect((await serverState(request)).state.channels['ch21']!.pan.lr).toBe(-0.06);
  await pan.focus();
  await page.keyboard.press('Alt+End');
  await settled(page);
  await expect(pan).toHaveAttribute('aria-valuenow', '1');
  await page.keyboard.press('Alt+0');
  await settled(page);
  await expect(pan).toHaveAttribute('aria-valuenow', '0');
});

test('sets the loudness target from the setup view and judges integrated loudness against it', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Setup' }).click();
  await page.getByRole('radio', { name: 'ATSC A/85' }).click();
  await settled(page);
  expect((await serverState(request)).state.metering.loudness).toEqual({ preset: 'atsc', target: -24, tolerance: 2 });
  const target = page.getByRole('textbox', { name: /^Target/ });
  await expect(target).toHaveValue('-24.0');
  await target.fill('-27');
  await target.press('Enter');
  await settled(page);
  expect((await serverState(request)).state.metering.loudness).toEqual({ preset: 'custom', target: -27, tolerance: 2 });
  await expect(page.getByRole('radio', { name: 'Custom' })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.locator('[data-master="pgm20"]').getByTitle('Target -27 LUFS +/-2 LU')).toHaveText('-27.0');
  await patch(request, '/metering/loudness/preset', 'ebu');
});

test('sets the LFE send and the stereo image of a channel from the processing panel', async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="st1"] .head').click();
  // A stereo channel is balanced by its pan; the surround panner's controls act on a mono one and are not offered.
  await expect(page.getByLabel('LFE', { exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Music front rear')).toHaveCount(0);
  await expect(page.getByText('Front/rear, spread and LFE are mono-channel controls.')).toBeVisible();
  await page.getByRole('radio', { name: 'Right to both', exact: true }).click();
  await settled(page);
  expect((await serverState(request)).state.channels['st1']!.image).toBe('rightToBoth');
  await expect(page.getByRole('radio', { name: 'Right to both', exact: true })).toHaveAttribute('aria-checked', 'true');
  await patch(request, '/channels/st1/image', 'normal');
  await patch(request, '/channels/ch1/routes/pgm51', false);
  await page.locator('[data-channel="ch1"] .head').click();
  // A mono channel that feeds no bus of more than two legs has no field to be placed in.
  await expect(page.getByLabel('LFE', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Front/rear, spread and LFE act on a surround bus: route this channel to one.')).toBeVisible();
  await patch(request, '/channels/ch1/routes/pgm51', true);
  await expect(page.getByLabel('LFE', { exact: true })).toBeVisible();
  await patch(request, '/channels/ch1/pan/lfeSend', -12);
  await expect(page.locator('[role="slider"][data-path="/channels/ch1/pan/lfeSend"]')).toHaveAttribute('aria-valuetext', '-12.0');
  // On the fader taper, as the Aux view's send levels: -12 dB sits at 47.5 % of the travel.
  await expect(page.locator('[role="slider"][data-path="/channels/ch1/pan/lfeSend"]')).toHaveAttribute('aria-valuenow', /^4[78]$/);
  await patch(request, '/channels/ch1/pan/lfeSend', -90);
  await patch(request, '/channels/ch1/routes/pgm51', true);
});

// The backend runs without an engine, so no channel there has a delay line.
test('enters a channel delay in frames and shows it in ms', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="ch2"] .head').click();
  await page.getByRole('radio', { name: 'frames' }).click();
  const entry = page.locator('input[data-path="/channels/ch2/delay"]');
  await entry.fill('3');
  await entry.press('Enter');
  await settled(page);
  expect((await serverState(request)).state.channels['ch2']!.delay).toBe(5760);
  await page.getByRole('radio', { name: 'milliseconds' }).click();
  await expect(entry).toHaveValue('120.0');
  await entry.fill('2.5 s');
  await entry.press('Enter');
  await settled(page);
  expect((await serverState(request)).state.channels['ch2']!.delay).toBe(120000);
  await patch(request, '/channels/ch2/delay', 0);
});

test('shows a channel delay range in the selected unit and caps an entry at the delay line', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="st1"] .head').click();
  const panel = page.getByRole('region', { name: 'Music processing' });
  await expect(panel.getByText('0 to 100.0 ms')).toBeVisible();
  await panel.getByRole('radio', { name: 'frames' }).click();
  await expect(panel.getByText('0 to 2.50 fr')).toBeVisible();
  const entry = panel.locator('input[data-path="/channels/st1/delay"]');
  await entry.fill('1 s');
  await entry.press('Enter');
  await settled(page);
  expect((await serverState(request)).state.channels['st1']!.delay).toBe(4800);
  await expect(entry).toHaveValue('2.50');
  await patch(request, '/channels/st1/delay', 0);
});

test('greys out the delay of a channel without a delay line and leaves a bus delay as it was', async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="st4"] .head').click();
  const panel = page.getByRole('region', { name: 'FX processing' });
  await expect(panel.locator('input[data-path="/channels/st4/delay"]')).toBeDisabled();
  await expect(panel.getByText('No delay line')).toBeVisible();
  await expect(panel.getByRole('radio', { name: 'frames' })).toBeDisabled();
  await panel.getByRole('button', { name: 'Close processing' }).click();
  await page.locator('[data-master="pgm20"] .head').click();
  const bus = page.getByRole('region', { name: 'PGM 2.0 processing' });
  const entry = bus.locator('input[data-path="/buses/pgm20/delay"]');
  await expect(entry).toBeEnabled();
  await expect(bus.getByText('No delay line')).toHaveCount(0);
  await entry.fill('40');
  await entry.press('Enter');
  await settled(page);
  expect((await serverState(request)).state.buses['pgm20']!.delay).toBe(1920);
  expect((await serverState(request)).state.channels['st4']!.delay).toBe(0);
  await patch(request, '/buses/pgm20/delay', 0);
});

test('switches a programme bus limiter and the dynamics order from the processing panel', async ({ page, request }) => {
  await live(page);
  expect((await serverState(request)).state.buses['pgm20']!.limiter).toEqual({ enabled: true, ceiling: -1 });
  await page.locator('[data-master="pgm20"] .head').click();
  await page.getByRole('button', { name: 'PGM 2.0 true-peak limiter' }).click();
  await page.getByRole('button', { name: 'EQ > DYN' }).click();
  await settled(page);
  const bus = (await serverState(request)).state.buses['pgm20']!;
  expect(bus.limiter.enabled).toBe(false);
  expect(bus.dynamicsBeforeEq).toBe(true);
  await patch(request, '/buses/pgm20/limiter/enabled', true);
  await patch(request, '/buses/pgm20/dynamicsBeforeEq', false);
});

test('sets AFV rise, hold, fall and closed level from the AFV view', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'AFV' }).click();
  const hold = page.getByRole('textbox', { name: /^Amb 1 AFV hold time/ });
  await hold.fill('300');
  await hold.press('Enter');
  const off = page.getByRole('textbox', { name: /^Amb 1 AFV closed level/ });
  await off.fill('-20');
  await off.press('Tab');
  await settled(page);
  expect((await serverState(request)).state.afv['ch13']).toMatchObject({ holdMs: 300, offLevel: -20 });
  await patch(request, '/afv/ch13/holdMs', 0);
  await patch(request, '/afv/ch13/offLevel', -90);
});

test('talks into a mix-minus output while TALK is held and stops on release', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Aux / N-1' }).click();
  await page.getByRole('combobox', { name: 'N-1 Phone 1 talkback source' }).selectOption('ch1');
  await settled(page);
  const talk = page.getByRole('button', { name: /^N-1 Phone 1 talk/ });
  const active = async () => (await serverState(request)).state.mixMinus['mm1']!.interrupt.active;
  await talk.hover();
  await page.mouse.down();
  await expect.poll(active).toBe(true);
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  await page.waitForTimeout(400);
  await page.mouse.up();
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners['/mixMinus/mm1/interrupt/active']).toBeUndefined();
  // A tap latches the talk until the next tap.
  await talk.click();
  await page.waitForTimeout(500);
  expect(await active()).toBe(true);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await talk.click();
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners['/mixMinus/mm1/interrupt/active']).toBeUndefined();
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

/** Aux / N-1 view with a talkback source on N-1 Phone 1: its TALK key and the server's interrupt/active. */
async function talkKey(page: Page, request: APIRequestContext) {
  await live(page);
  await page.getByRole('button', { name: 'Aux / N-1' }).click();
  const select = page.getByRole('combobox', { name: 'N-1 Phone 1 talkback source' });
  if ((await select.inputValue()) !== 'ch1') await select.selectOption('ch1');
  await settled(page);
  return {
    talk: page.getByRole('button', { name: /^N-1 Phone 1 talk/ }),
    active: async () => (await serverState(request)).state.mixMinus['mm1']!.interrupt.active,
  };
}

test('ends a latched talk from the notice when another session asks for it', async ({ page, request, baseURL }) => {
  const { talk, active } = await talkKey(page, request);
  const path = '/mixMinus/mm1/interrupt/active';
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await expect.poll(active).toBe(true);
  const other = await RawClient.open(baseURL!);
  try {
    const tag = handleTag(ownerHandle(other.session));
    other.send({ type: 'request', path });
    const notice = page.getByTestId('notice');
    await expect(notice).toContainText(`web client ${tag} asks for N-1 Phone 1 interrupt active`);
    await notice.getByRole('button', { name: 'RELEASE' }).click();
    await expect.poll(active).toBe(false);
    await expect.poll(async () => (await serverState(request)).owners[path]).toBeUndefined();
    await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
    await expect(talk).toHaveAttribute('aria-pressed', 'false');
    await expect(notice.getByRole('button', { name: 'RELEASE' })).toHaveCount(0);
  } finally {
    other.close();
    await patch(request, '/mixMinus/mm1/interrupt/source', null);
  }
});

test('ends the talk when a short touch on TALK is cancelled instead of latching it', async ({ page, request }) => {
  const { talk, active } = await talkKey(page, request);
  const box = (await talk.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: box.x + box.width / 2, y: box.y + box.height / 2 }] });
  await expect.poll(active).toBe(true);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] });
  await expect.poll(active).toBe(false);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  await expect(talk).toHaveAttribute('aria-pressed', 'false');
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('ends the talk when focus leaves TALK during a short Space press', async ({ page, request }) => {
  const { talk, active } = await talkKey(page, request);
  await talk.focus();
  await page.keyboard.down(' ');
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  // Tab moves focus away well within the latch time; the keyup lands elsewhere.
  await page.waitForTimeout(100);
  await page.keyboard.press('Tab');
  await page.keyboard.up(' ');
  await expect.poll(active).toBe(false);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  // A short Space press that ends on the key still latches.
  await talk.focus();
  await page.keyboard.press(' ');
  await page.waitForTimeout(400);
  expect(await active()).toBe(true);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await page.keyboard.press(' ');
  await expect.poll(active).toBe(false);
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

const TALK_OWNER = '/mixMinus/mm1/interrupt/active';

test('ends a held talk when its view is left during the press', async ({ page, request }) => {
  const { talk, active } = await talkKey(page, request);
  const box = (await talk.boundingBox())!;
  const at = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [at] });
  await expect.poll(active).toBe(true);
  // Held past the latch time: momentary. Another view replaces the key
  // under the finger, so no lift ever reaches it.
  await page.waitForTimeout(400);
  await page.getByRole('button', { name: 'Scenes' }).click();
  await expect(talk).toHaveCount(0);
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners[TALK_OWNER]).toBeUndefined();
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await page.waitForTimeout(400);
  expect(await active()).toBe(false);
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('ends a latched talk when the page is hidden and says so on return, moving nothing', async ({ page, request }) => {
  // A mouse hold over a tab switch gets no blur on Safari. Playwright
  // cannot hide a headless page or fire a real pagehide without leaving
  // it, so the page reports itself hidden and fires visibilitychange.
  await page.addInitScript(() => {
    const w = window as unknown as { __shift: number };
    w.__shift = 0;
    new PerformanceObserver((list) => {
      for (const e of list.getEntries() as unknown as Array<{ value: number }>) w.__shift += e.value;
    }).observe({ type: 'layout-shift', buffered: true });
  });
  const visibility = (state: 'hidden' | 'visible') =>
    page.evaluate((v) => {
      Object.defineProperty(document, 'visibilityState', { value: v, configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    }, state);
  const { talk, active } = await talkKey(page, request);
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await expect.poll(active).toBe(true);
  await visibility('hidden');
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners[TALK_OWNER]).toBeUndefined();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  // Back on the page: a notice says the talk ended and why.
  const text = 'N-1 Phone 1 talk ended: the page was hidden';
  const notice = page.getByTestId('notice');
  await expect(page.getByText(text)).toHaveCount(0);
  await visibility('visible');
  await expect(notice).toContainText(text);
  // It takes the fixed notice line: nothing on the page moved, from load on.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  expect(await page.evaluate(() => (window as unknown as { __shift: number }).__shift)).toBe(0);
  // Cut off or not, its whole text is in a tooltip that the keyboard reaches.
  await notice.focus();
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Tab');
  await expect(page.getByRole('tooltip')).toHaveText(text);
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('ends a latched talk when its view is left, so another client can talk', async ({ page, browser, request }) => {
  const { talk, active } = await talkKey(page, request);
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await expect.poll(active).toBe(true);
  await page.getByRole('button', { name: 'Scenes' }).click();
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners[TALK_OWNER]).toBeUndefined();
  await page.getByRole('button', { name: 'Aux / N-1' }).click();
  await expect(talk).toHaveAttribute('aria-pressed', 'false');
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  const other = await browser.newContext();
  const second = await talkKey(await other.newPage(), request);
  await second.talk.click();
  await expect(second.talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await expect.poll(active).toBe(true);
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  await second.talk.click();
  await expect.poll(active).toBe(false);
  await other.close();
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('a tap on a lit TALK key stops the talk, or names the client that holds it', async ({ page, browser, request }) => {
  const { talk, active } = await talkKey(page, request);
  // Lit without an owner (a script): one tap here stops it.
  await patch(request, '/mixMinus/mm1/interrupt/active', true);
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  await talk.click();
  await expect.poll(active).toBe(false);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  // Latched on a second client, which owns it: a tap here does not
  // latch it here too; it names the holder, and the holder stops it.
  const other = await browser.newContext();
  const page2 = await other.newPage();
  const second = await talkKey(page2, request);
  await second.talk.click();
  await expect(second.talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  await talk.click();
  await expect(page.getByTestId('notice')).toContainText('N-1 Phone 1 talk is held by');
  await page.waitForTimeout(400);
  expect(await active()).toBe(true);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  await second.talk.click();
  await expect.poll(active).toBe(false);
  await expect(talk).toHaveAttribute('aria-pressed', 'false');
  await other.close();
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('a latched talk stopped while writes are locked never shows as unlatched while it is still on', { tag: '@mock' }, async ({ page, request }) => {
  test.setTimeout(60_000);
  const { talk, active } = await talkKey(page, request);
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  // Every state the key shows from here: lit means latched, until it goes dark.
  await talk.evaluate((el) => {
    const seen: string[] = [];
    (window as unknown as { __talk: string[] }).__talk = seen;
    const note = () => seen.push(`${el.getAttribute('aria-pressed')} ${el.getAttribute('aria-label')}`);
    new MutationObserver(note).observe(el, { attributes: true, attributeFilter: ['aria-pressed', 'aria-label'] });
  });
  const primary = (await debug(page, 'primaryReplica'))!;
  const others = REPLICAS.filter((r) => r !== primary);
  for (const r of others) expect((await request.post(`/__mock/replicas/${r}/partition`)).ok()).toBe(true);
  await expect(page.getByTestId('quorum')).toHaveAttribute('data-quorum', 'lost');
  // The press sends the release, which the server applies once it can
  // commit; without quorum the session's lease also runs out. Either way
  // the key stays latched until the state says the talk is off.
  await talk.click({ force: true });
  for (const r of others) expect((await request.post(`/__mock/replicas/${r}/heal`)).ok()).toBe(true);
  await expect.poll(active, { timeout: 10_000 }).toBe(false);
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  await expect(talk).toHaveAttribute('aria-pressed', 'false');
  const seen = await page.evaluate(() => (window as unknown as { __talk: string[] }).__talk);
  expect(seen.filter((x) => x === 'true N-1 Phone 1 talk')).toEqual([]);
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('reads loudness in LU relative to the target on request', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const integrated = page.getByTestId('loudness-i-pgm20');
  await expect(integrated).not.toHaveText(/--\.-/, { timeout: 10_000 });
  await expect(integrated).toHaveText(/^\s*-\d/);
  await page.getByRole('button', { name: 'Setup' }).click();
  await page.getByRole('radio', { name: 'LU', exact: true }).click();
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.getByRole('group', { name: 'Loudness in LU relative to -23 LUFS' }).first()).toBeVisible();
  // The mock programme sits near the target, so it reads a few LU, signed.
  await expect(integrated).toHaveText(/^\s*[+-]\d{1,2}\.\d$/);
  const lu = Number((await integrated.textContent())!.trim());
  expect(Math.abs(lu)).toBeLessThan(20);
  await page.getByRole('button', { name: 'Setup' }).click();
  await page.getByRole('radio', { name: 'LUFS', exact: true }).click();
});

test('keeps the alignment level below the permitted maximum', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Setup' }).click();
  const alignment = page.getByRole('textbox', { name: /^Alignment/ });
  await alignment.fill('-3');
  await alignment.press('Enter');
  await settled(page);
  const zones = (await serverState(request)).state.metering.zones;
  expect(zones.alignment).toBeLessThan(zones.permittedMax);
  await patch(request, '/metering/zones/alignment', -18);
});

test('puts line-up tone at the selected level on chosen outputs from the setup view', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Setup' }).click();
  await page.getByRole('radio', { name: '-20 dBFS' }).click();
  await page.getByRole('button', { name: 'Tone on M&E' }).click();
  await page.getByRole('button', { name: 'Line-up tone', exact: true }).click();
  await settled(page);
  const tone = (await serverState(request)).state.tone;
  expect(tone).toMatchObject({ on: true, level: -20, targets: ['me'] });
  await expect(page.getByRole('button', { name: 'Line-up tone', exact: true })).toHaveAttribute('aria-pressed', 'true');
  // The header and the targeted master say so in every view.
  await expect(page.getByTestId('tone')).toBeVisible();
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.getByTestId('tone-me')).toBeVisible();
  await expect(page.getByTestId('tone-pgm20')).toBeHidden();
  await patch(request, '/tone/on', false);
  await expect(page.getByTestId('tone')).toBeHidden();
  await expect(page.getByTestId('tone-me')).toBeHidden();
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('labels every leg of a 5.1 master under its own bar', async ({ page }) => {
  await live(page);
  const master = page.locator('[data-master="pgm51"]');
  const geometry = await master.evaluate((el) => {
    const m = el.querySelector<HTMLElement>('[data-meter]')!.getBoundingClientRect();
    return { x: m.x, w: m.width, labels: [...el.querySelectorAll<HTMLElement>('.legs span')].map((s) => {
      const r = s.getBoundingClientRect();
      const range = document.createRange();
      range.selectNodeContents(s);
      const t = range.getBoundingClientRect();
      return { text: s.textContent, centre: r.x + r.width / 2, left: t.left, right: t.right };
    }) };
  });
  // Bar geometry of meters/layer.ts: an 18 px scale, then equal bars 1 px apart.
  const n = geometry.labels.length;
  expect(geometry.labels.map((l) => l.text)).toEqual(['L', 'R', 'C', 'LFE', 'Ls', 'Rs']);
  const bw = (geometry.w - 18 - (n - 1)) / n;
  geometry.labels.forEach((l, i) => {
    const left = geometry.x + 18 + i * (bw + 1);
    expect(l.centre, l.text!).toBeGreaterThanOrEqual(left);
    expect(l.centre, l.text!).toBeLessThanOrEqual(left + bw);
    // The text itself stays inside its column: no label touches the next.
    expect(l.left, l.text!).toBeGreaterThanOrEqual(left - 0.5);
    expect(l.right, l.text!).toBeLessThanOrEqual(left + bw + 0.5);
  });
});

test('latches a true-peak over of a channel direct out until cleared', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const overs = page.getByTestId('channel-overs');
  const pres1 = /Pres 1(,|$|\.)/;
  // Other channels of the mock may latch their own overs meanwhile.
  await patch(request, '/channels/ch1/trim', -20);
  await page.waitForTimeout(200);
  if (await overs.isEnabled()) await overs.click();
  await expect(overs).not.toHaveAccessibleName(pres1);
  await patch(request, '/channels/ch1/trim', 20);
  await expect(overs).toHaveAccessibleName(pres1);
  await patch(request, '/channels/ch1/trim', -20);
  await page.waitForTimeout(300);
  await expect(overs).toHaveAccessibleName(pres1);
  await overs.click();
  await expect(overs).not.toHaveAccessibleName(pres1);
  await patch(request, '/channels/ch1/trim', 0);
});

test('notes the gaps and a restarted start of a loudness measurement beside I and LRA, with a tooltip saying what each means', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  // VT 2, the mock's faulty input, feeds every bus: each counts its slices as gaps.
  const me = page.getByTestId('loudness-note-me');
  await expect(me.locator('.gaps')).toHaveText(/^\d+k? gaps?$/, { timeout: 5000 });
  const box = (await me.boundingBox())!;
  // A reset starts me's measurement on every replica: nothing restarted.
  const reset = await request.post('/api/v1/loudness/reset', { data: { ids: ['me'] }, headers: { 'X-Samplez-Request': '1' } });
  expect(reset.status()).toBe(204);
  await expect(me.locator('.since')).toHaveText('', { timeout: 5000 });
  // Every replica back after it, one at a time: each measures from its own
  // start and never applied the reset the state counts.
  for (const id of REPLICAS) {
    expect((await request.post(`/__mock/replicas/${id}/kill`)).ok()).toBe(true);
    expect((await request.post(`/__mock/replicas/${id}/restart`)).ok()).toBe(true);
  }
  await expect(me.locator('.since')).toHaveText(/^since \d\d:\d\d$/, { timeout: 15_000 });
  await me.hover();
  const tip = page.locator('#faderz-tip');
  await expect(tip).toContainText('an input was down, or this mixer instance could not measure the audio, for example while the server was overloaded');
  await expect(tip).toContainText("can make its count and its reading differ slightly from the other instances'");
  await expect(tip).toContainText("This mixer instance's measurement restarted at");
  await expect(tip).toContainText("earlier programme loudness isn't included");
  // The note keeps its box, whatever it says.
  expect(await me.boundingBox()).toEqual(box);
  // The integrated loudness is still read, never blanked for the gaps.
  await expect(page.getByTestId('loudness-i-me')).not.toHaveText(/--\.-/, { timeout: 10_000 });
});

test('latches a true-peak over on a master until RESET, however short it was', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const lamp = page.getByTestId('tp-over-pgm20');
  await expect(lamp).not.toHaveClass(/over/);
  // Tone at 0 dBFS bypasses the limiter: true peak 0 dBTP, above -1.
  await patch(request, '/tone/level', 0);
  await patch(request, '/tone/targets', ['pgm20']);
  await patch(request, '/tone/on', true);
  await page.waitForTimeout(150);
  await patch(request, '/tone/on', false);
  await expect(lamp).toHaveClass(/over/);
  await page.waitForTimeout(500);
  await expect(lamp).toHaveClass(/over/);
  await holdReset(page, 'pgm20');
  await expect(lamp).not.toHaveClass(/over/);
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('shows a channel true peak only on channels with a direct out', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const { shape } = await serverState(request);
  const direct = new Set(shape.outputs.map((o) => o.source));
  const without = shape.channels.find((c) => !direct.has(c.id))!;
  const overs = page.getByTestId('channel-overs');
  await patch(request, `/channels/${without.id}/trim`, 20);
  await patch(request, '/channels/ch1/trim', 20);
  await expect(overs).toHaveAccessibleName(/Pres 1(,|$|\.)/);
  await page.waitForTimeout(500);
  expect(await overs.getAttribute('aria-label')).not.toContain(without.name);
  await patch(request, `/channels/${without.id}/trim`, 0);
  await patch(request, '/channels/ch1/trim', 0);
});

test('reads the highest true peak since RESET, not the last tenth of a second', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const tp = page.getByTestId('tp-pgm20');
  await holdReset(page, 'pgm20');
  await patch(request, '/tone/level', 0);
  await patch(request, '/tone/targets', ['pgm20']);
  await patch(request, '/tone/on', true);
  await page.waitForTimeout(150);
  await patch(request, '/tone/on', false);
  await expect(tp).toHaveText(/^\s*-?0\.\d$/);
  await page.waitForTimeout(1500);
  await expect(tp).toHaveText(/^\s*-?0\.\d$/);
  await holdReset(page, 'pgm20');
  await expect(tp).not.toHaveText(/^\s*-?0\.\d$/);
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('a RESET on one screen clears the true-peak over on every screen', { tag: '@meters' }, async ({ page, browser, request }) => {
  await live(page);
  const other = await browser.newContext();
  const page2 = await other.newPage();
  await live(page2);
  await patch(request, '/tone/level', 0);
  await patch(request, '/tone/targets', ['pgm20']);
  await patch(request, '/tone/on', true);
  await page.waitForTimeout(150);
  await patch(request, '/tone/on', false);
  await expect(page.getByTestId('tp-over-pgm20')).toHaveClass(/over/);
  await expect(page2.getByTestId('tp-over-pgm20')).toHaveClass(/over/);
  // The 150 ms of tone leaves the 400 ms momentary window before RESET,
  // as it would on a desk; a RESET while the momentary level is at its
  // maximum shows only in integrated and LRA (unit tests).
  await page.waitForTimeout(600);
  await holdReset(page, 'pgm20');
  await expect(page.getByTestId('tp-over-pgm20')).not.toHaveClass(/over/);
  await expect(page2.getByTestId('tp-over-pgm20')).not.toHaveClass(/over/);
  await expect(page2.getByTestId('tp-pgm20')).not.toHaveText(/^\s*-?0\.\d$/);
  await other.close();
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('mutes and solos monitor speakers and sets monitor image and dim depth', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Mute speaker R' }).click();
  await page.getByRole('button', { name: 'Solo speaker L' }).click();
  await page.getByRole('radio', { name: 'Monitor left to both' }).click();
  await settled(page);
  expect((await serverState(request)).state.monitor).toMatchObject({ speakerMute: [false, true], speakerSolo: [true, false], image: 'leftToBoth' });
  await page.getByRole('button', { name: 'Setup' }).click();
  const depth = page.getByRole('textbox', { name: /^Dim depth/ });
  await depth.fill('12');
  await depth.press('Enter');
  await page.getByRole('radio', { name: 'L/R only' }).click();
  await settled(page);
  expect((await serverState(request)).state.monitor).toMatchObject({ dimDepth: 12, downmix: 'lr' });
  for (const [path, value] of [
    ['/monitor/speakerMute/1', false],
    ['/monitor/speakerSolo/0', false],
    ['/monitor/image', 'normal'],
    ['/monitor/dimDepth', 20],
    ['/monitor/downmix', 'loRo'],
  ] as const) await patch(request, path, value);
});

test('meters the listen sum on the monitor while a PFL is on, PFL over AFL', async ({ page, request }) => {
  await live(page);
  const meter = page.locator('[data-meter="monitor/mon"]');
  await expect(meter).toHaveAttribute('aria-label', /source/);
  await patch(request, '/channels/ch3/solo', 'afl');
  await expect(meter).toHaveAttribute('aria-label', 'Monitor meter, AFL listen sum');
  await patch(request, '/channels/ch1/solo', 'pfl');
  await expect(meter).toHaveAttribute('aria-label', 'Monitor meter, PFL listen sum');
  await expect(page.locator('.listen b')).toHaveText('PFL (AFL held)');
  await patch(request, '/channels/ch1/solo', 'off');
  await patch(request, '/channels/ch3/solo', 'off');
  await expect(meter).toHaveAttribute('aria-label', /source/);
});

test('listens to every member of a VCA with its PFL key', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'VCA PRES PFL, all members' }).click();
  await settled(page);
  const state = (await serverState(request)).state;
  expect(state.vcas['vca1']!.solo).toBe('pfl');
  expect(state.channels['ch1']!.solo).toBe('off');
  await expect(page.getByRole('region', { name: 'Monitor' }).getByText('VCA PRES')).toBeVisible();
  await patch(request, '/vcas/vca1/solo', 'off');
});

test('switches a presenter channel to its B input and marks the strip', async ({ page, request }) => {
  await live(page);
  const badge = page.locator('[data-channel="ch1"] .badge.alt');
  await expect(badge).toBeHidden();
  await page.locator('[data-channel="ch1"] .head').click();
  await page.getByRole('radio', { name: 'Input B' }).click();
  await settled(page);
  expect((await serverState(request)).state.channels['ch1']!.input).toBe('b');
  await expect(badge).toBeVisible();
  await patch(request, '/channels/ch1/input', 'a');
});

test('mutes a programme output only after a hold, and a short press does nothing', async ({ page, request }) => {
  await live(page);
  const key = page.getByRole('button', { name: 'PGM 2.0 mute, hold to mute' });
  await key.hover();
  await page.mouse.down();
  await page.waitForTimeout(150);
  await page.mouse.up();
  await page.waitForTimeout(600);
  await settled(page);
  expect((await serverState(request)).state.buses['pgm20']!.mute).toBe(false);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await settled(page);
  expect((await serverState(request)).state.buses['pgm20']!.mute).toBe(true);
  await expect(key).toHaveAttribute('aria-pressed', 'true');
  await key.click();
  await settled(page);
  expect((await serverState(request)).state.buses['pgm20']!.mute).toBe(false);
});

test('types an EQ frequency and a gate threshold into the processing panel', async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="ch5"] .head').click();
  const freq = page.getByRole('textbox', { name: 'Freq value, type to set' }).nth(1);
  await freq.fill('1.2k');
  await freq.press('Enter');
  const thr = page.getByRole('textbox', { name: 'Thr value, type to set' }).first();
  await thr.fill('-42');
  await thr.press('Enter');
  await settled(page);
  const ch = (await serverState(request)).state.channels['ch5']!;
  expect(ch.eq.bands[1]!.freq).toBe(1200);
  expect(ch.gate.threshold).toBe(-42);
  await patch(request, '/channels/ch5/eq/bands/1/freq', 400);
  await patch(request, '/channels/ch5/gate/threshold', -50);
});

test('Escape in a typed field restores its value and leaves the processing panel open', async ({ page, request }) => {
  await live(page);
  await page.locator('[data-channel="ch5"] .head').click();
  const panel = page.getByRole('region', { name: 'Guest 3 processing' });
  // ch5 has a delay line only on the mock: the backend's channels have none, so their delay field is disabled.
  const delay = panel.getByRole('textbox', { name: /delay in/i });
  for (const field of [page.getByRole('textbox', { name: 'Thr value, type to set' }).first(), ...(BACKEND ? [] : [delay])]) {
    const before = await field.inputValue();
    await field.fill('-12');
    await field.press('Escape');
    await expect(panel).toBeVisible();
    await expect(field).toHaveValue(before);
  }
  expect((await serverState(request)).state.channels['ch5']!.gate.threshold).toBe(-50);
  // Escape outside a field still closes it.
  await panel.getByRole('button', { name: 'Close processing' }).focus();
  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
});

test('keeps the processing panel open on a second press and steps to the next and previous channel', async ({ page }) => {
  await live(page);
  const head = (id: string) => page.locator(`[data-channel="${id}"] .head`);
  const panel = (name: string) => page.getByRole('region', { name: `${name} processing` });
  await head('ch1').click();
  await expect(panel('Pres 1')).toBeVisible();
  await head('ch1').click();
  await expect(panel('Pres 1')).toBeVisible();
  await expect(head('ch1')).toHaveAttribute('aria-pressed', 'true');
  await page.getByRole('button', { name: 'Next channel' }).click();
  await expect(panel('Pres 2')).toBeVisible();
  await expect(head('ch2')).toHaveAttribute('aria-pressed', 'true');
  await expect(head('ch1')).toHaveAttribute('aria-pressed', 'false');
  await page.getByRole('button', { name: 'Previous channel' }).click();
  await expect(panel('Pres 1')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Previous channel' })).toBeDisabled();
  await page.getByRole('button', { name: 'Close processing' }).click();
  await expect(panel('Pres 1')).toHaveCount(0);
});

test('locks the desk with a hold so nothing on the screen changes the mixer, and marks recall-safe strips', async ({ page, request }) => {
  await live(page);
  const lock = page.getByTestId('desk-lock');
  // Its text tells it from the red LOCK of a link that sends no commands.
  await expect(lock).toHaveText('LOCK DESK');
  // A press shorter than the hold does nothing.
  await lock.click();
  await expect(lock).toHaveAttribute('aria-pressed', 'false');
  const box = (await lock.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect(lock).toHaveAttribute('aria-pressed', 'true');
  await expect(lock).toHaveText('LOCKED');
  const mute = page.getByRole('button', { name: 'Pres 1 mute' });
  const before = (await serverState(request)).state.channels['ch1']!.mute;
  await mute.click({ force: true });
  await page.waitForTimeout(500);
  expect((await serverState(request)).state.channels['ch1']!.mute).toBe(before);
  await expect(mute).toHaveAttribute('aria-disabled', 'true');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect(lock).toHaveAttribute('aria-pressed', 'false');
  await expect(mute).toHaveAttribute('aria-disabled', 'false');

  await patch(request, '/recallSafe/ch3', ['fader', 'mute']);
  await expect(page.getByTestId('iso-ch3')).toBeVisible();
  // The groups are in the head's tooltip and accessible name, which the
  // keyboard and a long touch reach as well as a hover.
  const head = page.locator('[data-channel="ch3"] .head');
  await expect(head).toHaveAccessibleName(/, recall safe: fader, mute$/);
  await expect(head).toHaveAttribute('data-tip', 'Guest 1 (Studio mic 3); recall safe: fader, mute');
  await head.focus();
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Tab');
  await expect(page.getByRole('tooltip')).toHaveText('Guest 1 (Studio mic 3); recall safe: fader, mute');
  await patch(request, '/recallSafe/ch3', []);
  await expect(page.getByTestId('iso-ch3')).toBeHidden();
});

test('labels the channel meter in dBFS on its outer side, apart from the fader scale', async ({ page }) => {
  // Each label in page coordinates, from the canvas's box when it was
  // drawn: the meter layer moves its canvas over the visible meters.
  await page.addInitScript(() => {
    const seen: Array<{ text: string; x: number; y: number }> = [];
    (window as unknown as { __labels: typeof seen }).__labels = seen;
    const fill = CanvasRenderingContext2D.prototype.fillText;
    CanvasRenderingContext2D.prototype.fillText = function (text: string, x: number, y: number, max?: number) {
      const m = this.getTransform();
      const c = this.canvas.getBoundingClientRect();
      seen.push({ text, x: c.x + (x * m.a) / devicePixelRatio, y: c.y + (y * m.d) / devicePixelRatio });
      return max === undefined ? fill.call(this, text, x, y) : fill.call(this, text, x, y, max);
    };
  });
  await live(page);
  /** Label texts drawn in the 17 px column at the left or right edge of a meter. */
  const column = async (meter: string, side: 'left' | 'right') => {
    const m = (await page.locator(`[data-meter="${meter}"]`).boundingBox())!;
    return page.evaluate(
      ({ m, side }) => {
        const x0 = side === 'left' ? m.x : m.x + m.width - 17;
        return [...new Set((window as unknown as { __labels: Array<{ text: string; x: number; y: number }> }).__labels
          .filter((l) => l.x >= x0 && l.x <= x0 + 17 && l.y >= m.y && l.y <= m.y + m.height)
          .map((l) => l.text))].sort();
      },
      { m, side },
    );
  };
  // A channel meter: full scale, the zone boundaries and -40, on its outer side.
  await expect.poll(() => column('channels/ch1', 'right')).toEqual(['-18', '-40', '-9', '0']);
  // A master meter: every mark, on its left.
  await expect.poll(() => column('buses/pgm20', 'left')).toEqual(['-18', '-30', '-40', '-50', '-60', '-9', '0']);
});

test('moves faders at a tenth of their travel while FINE is lit, without a modifier key', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch2/fader', 0);
  await expect(page.getByTestId('fader-value-ch2')).toHaveText(formatDb(0));
  await page.getByRole('button', { name: 'FINE' }).click();
  await dragFader(page, '/channels/ch2/fader', 100);
  await settled(page);
  const fine = await fader(request, 'ch2');
  expect(fine).toBeLessThan(0);
  expect(fine).toBeGreaterThan(-1.5);
  await page.getByRole('button', { name: 'FINE' }).click();
  await dragFader(page, '/channels/ch2/fader', 100);
  await settled(page);
  expect(await fader(request, 'ch2')).toBeLessThan(fine - 3);
});

test('opens processing at the section of a strip chip and assigns a bus from the strip with a hold', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Guest 1 Gate,', exact: false }).click();
  const panel = page.getByRole('region', { name: 'Guest 1 processing' });
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('heading', { name: 'Gate' })).toBeInViewport();

  const chip = page.locator('[data-path="/channels/ch1/routes/pgm51"]');
  const before = (await serverState(request)).state.channels['ch1']!.routes['pgm51'];
  await chip.click();
  await page.waitForTimeout(400);
  expect((await serverState(request)).state.channels['ch1']!.routes['pgm51']).toBe(before);
  const box = (await chip.boundingBox())!;
  expect(box.height + 6).toBeGreaterThanOrEqual(24);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect.poll(async () => (await serverState(request)).state.channels['ch1']!.routes['pgm51']).toBe(!before);
  await expect(chip).toHaveAttribute('aria-pressed', String(!before));
});

test('shows the source under the channel name and colours the scribble strip from the processing panel', async ({ page, request }) => {
  await live(page);
  const head = page.locator('[data-channel="ch3"] .head');
  await expect(head.locator('.source')).toHaveText('Studio mic 3');
  const name = (await head.locator('.name').evaluate((el) => parseFloat(getComputedStyle(el).fontSize)));
  expect(name).toBeGreaterThanOrEqual(15);
  const before = (await head.boundingBox())!.height;
  await head.click();
  await page.getByRole('radiogroup', { name: 'Guest 1 strip colour' }).getByRole('radio', { name: 'red' }).click();
  await expect(head).toHaveAttribute('data-color', 'red');
  await expect.poll(async () => (await serverState(request)).state.channels['ch3']!.color).toBe('red');
  expect((await head.boundingBox())!.height).toBe(before);
  await page.getByRole('radiogroup', { name: 'Guest 1 strip colour' }).getByRole('radio', { name: 'No colour' }).click();
  await expect.poll(async () => (await serverState(request)).state.channels['ch3']!.color).toBe('none');
});

test('opens the engine counters above the lamp, inside the viewport, at desktop and phone widths', async ({ page }) => {
  await live(page);
  const lamp = page.getByTestId('engine');
  const counters = page.getByRole('region', { name: 'Engine counters' });
  for (const width of [1600, 390, 320]) {
    await page.setViewportSize({ width, height: 800 });
    await lamp.click();
    await expect(counters).toBeVisible();
    const b = (await lamp.boundingBox())!;
    const p = (await counters.boundingBox())!;
    // Fully on screen, and above the lamp rather than over it.
    expect(p.x, `${width}: left edge`).toBeGreaterThanOrEqual(0);
    expect(p.x + p.width, `${width}: right edge`).toBeLessThanOrEqual(width);
    expect(p.y, `${width}: top edge`).toBeGreaterThanOrEqual(0);
    expect(p.y + p.height, `${width}: above the lamp`).toBeLessThanOrEqual(b.y + 0.5);
    // Centred on the lamp, or moved in against a viewport edge.
    const centre = p.x + p.width / 2;
    const clamped = p.x <= 8.5 || p.x + p.width >= width - 8.5;
    expect(clamped || (centre >= b.x && centre <= b.x + b.width), `${width}: centre ${centre} over ${b.x}..${b.x + b.width}`).toBe(true);
    expect(p.x, `${width}: spans the lamp`).toBeLessThanOrEqual(b.x + b.width);
    expect(p.x + p.width, `${width}: spans the lamp`).toBeGreaterThanOrEqual(b.x);
    await lamp.click();
    await expect(counters).toHaveCount(0);
  }
});

test('shows one engine lamp in the status bar and the counters only on request', { tag: '@meters' }, async ({ page }) => {
  await live(page);
  const lamp = page.getByTestId('engine');
  await expect(lamp).toHaveText(/ENGINE (OK|WARN \d+)|WARN IN \S/);
  const width = (await lamp.boundingBox())!.width;
  await expect(page.locator('footer.bar')).not.toContainText('Late output blocks');
  await lamp.click();
  const counters = page.getByRole('region', { name: 'Engine counters' });
  // Every counter is listed in the panel by its name for people: none is cut.
  for (const name of COUNTER_NAMES) {
    await expect(counters.locator(`[data-counter="${name}"] .label`)).toHaveText(counterInfo(name).label);
  }
  await lamp.click();
  await expect(counters).toHaveCount(0);
  expect((await lamp.boundingBox())!.width).toBe(width);
  // OK, WARN with any count, two digits and capped ones included, and
  // WARN naming one input or several, a long name included, give the
  // lamp one width and fit its text: a change moves nothing.
  const texts = [...[0, 3, 12, 99, 250].map((n) => engineLamp(n)), engineLamp(0, ['VT 2']), engineLamp(1, ['A very long strip name', 'VT 2', 'x']), engineLamp(1, ['VT 2', ...Array.from({ length: 120 }, (_, i) => `ch${i}`)])];
  const sizes = await lamp.evaluate((b, texts) => {
    const text = b.querySelector<HTMLElement>('.fixed')!;
    const was = text.textContent;
    const out = texts.map((t) => {
      text.textContent = t;
      return { w: b.getBoundingClientRect().width, fits: text.scrollWidth <= text.getBoundingClientRect().width + 0.5 };
    });
    text.textContent = was;
    return out;
  }, texts);
  for (const s of sizes) expect(s).toEqual({ w: width, fits: true });
});

test('names the input at fault and why in the engine lamp and its panel, and explains each counter on a click', { tag: '@mock' }, async ({ page }) => {
  await live(page);
  const lamp = page.getByTestId('engine');
  // The mock prunes VT 2 (st3): the lamp names it within a counters period or two.
  await expect(lamp).toHaveText(/^\s*WARN IN VT 2/, { timeout: 5000 });
  // Its tooltip says why.
  await expect(lamp).toHaveAttribute('data-tip', /^Input VT 2 \(st3\): its flow delivers no audio in time/);
  await lamp.click();
  const counters = page.getByRole('region', { name: 'Engine counters' });
  await expect(counters.getByTestId('engine-faults').locator('[data-input="st3"]')).toContainText('Input VT 2 (st3): its flow delivers no audio in time');
  // The silenced blocks rose, amber; the expected ones never are.
  await expect(counters.locator('[data-counter="input_silenced_blocks_total"]')).toHaveClass(/nz/);
  await expect(counters.locator('.c.nz')).not.toHaveCount(0);
  for (const name of COUNTER_NAMES.filter((n) => !counterInfo(n).fault)) {
    await expect(counters.locator(`[data-counter="${name}"]`)).not.toHaveClass(/nz/);
  }
  // A click on (i) explains the counter and names its key; another closes it.
  const info = counters.getByRole('button', { name: 'What Silenced input blocks means' });
  await expect(info).toHaveAttribute('aria-expanded', 'false');
  await info.click();
  await expect(info).toHaveAttribute('aria-expanded', 'true');
  const help = counters.getByTestId('counter-help');
  await expect(help).toContainText(counterInfo('input_silenced_blocks_total').help);
  // Named among the inputs behind it (the mock adds a late block on another now and then).
  await expect(help).toContainText(/From (.*, )?VT 2 \(st3\)/);
  await expect(help).toContainText('input_silenced_blocks_total');
  // Keyboard: Enter on another (i) moves the explanation there.
  await counters.getByRole('button', { name: 'What Late output blocks means' }).focus();
  await page.keyboard.press('Enter');
  await expect(help).toContainText(counterInfo('deadline_miss_total').help);
  await expect(counters.getByTestId('counter-help')).toHaveCount(1);
  // Escape closes it and leaves focus on its button; Space opens it as Enter does.
  const late = counters.getByRole('button', { name: 'What Late output blocks means' });
  await page.keyboard.press('Escape');
  await expect(counters.getByTestId('counter-help')).toHaveCount(0);
  await expect(late).toHaveAttribute('aria-expanded', 'false');
  await expect(late).toBeFocused();
  await expect(counters).toBeVisible();
  await page.keyboard.press('Space');
  await expect(late).toHaveAttribute('aria-expanded', 'true');
  await expect(help).toContainText(counterInfo('deadline_miss_total').help);
  // The rise and the total sit under column headers.
  await expect(counters.locator('.cols')).toHaveText(/last 10 s\s*total/);
  // The panel stays inside the viewport with the explanation open.
  const p = (await counters.boundingBox())!;
  expect(p.y).toBeGreaterThanOrEqual(0);
  expect(p.x + p.width).toBeLessThanOrEqual(page.viewportSize()!.width);
});

test('lights PFL amber and every generic on key in the neutral colour, with no lit OFF key', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch1/routes/pgm51', true);
  await patch(request, '/channels/ch1/solo', 'pfl');
  const bg = (sel: string) => page.locator(sel).first().evaluate((el) => getComputedStyle(el).backgroundColor);
  const token = (name: string) =>
    page.evaluate((n) => {
      const probe = document.createElement('i');
      probe.style.color = `var(${n})`;
      document.body.append(probe);
      const c = getComputedStyle(probe).color;
      probe.remove();
      return c;
    }, name);
  await expect(page.getByRole('button', { name: 'Pres 1 PFL' })).toHaveAttribute('aria-pressed', 'true');
  expect(await bg('[data-channel="ch1"] .key.pfl')).toBe(await token('--listen'));
  expect(await bg('[data-path="/channels/ch1/routes/pgm51"]')).toBe(await token('--on'));
  expect(await token('--on')).not.toBe(await token('--accent'));
  await page.locator('[data-channel="ch1"] .head').click();
  const panel = page.getByRole('region', { name: 'Pres 1 processing' });
  await expect(panel.getByRole('button', { name: 'off', exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: 'pfl', exact: true }).click();
  await expect.poll(async () => (await serverState(request)).state.channels['ch1']!.solo).toBe('off');
});

test('writes the tally on the strip in words: PGM, PVW, and CLOSED when the AFV gate holds the channel down', async ({ page, request }) => {
  await live(page);
  const tally = (sources: Record<string, string>) =>
    request.put('/api/v1/afv/tally', { headers: { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' }, data: { sources } });
  await tally({ CAM1: 'program', CAM2: 'preview', CAM3: 'off' });
  await expect(page.getByTestId('tally-ch13')).toHaveText(/^\s*PGM CAM1\s*$/);
  await expect(page.getByTestId('tally-ch14')).toHaveText(/^\s*PVW CAM2\s*$/);
  await expect.poll(async () => (await serverState(request)).state.afv['ch15']!.open).toBe(false);
  await expect(page.getByTestId('tally-ch15')).toHaveText(/^\s*CLOSED CAM3\s*$/);
  await expect(page.getByTestId('tally-ch15')).not.toHaveCSS('text-decoration-line', 'line-through');
  // The words take the chip's colour in both themes: white on the PGM fill, the PVW colour on its tint.
  for (const theme of ['dark', 'light']) {
    if (theme === 'light') await switchTheme(page, 'light');
    for (const ch of ['ch13', 'ch14']) {
      const chip = page.getByTestId(`tally-${ch}`);
      await expect(chip.locator('span')).toHaveCSS('color', await chip.evaluate((el) => getComputedStyle(el).color));
    }
    await expect(page.getByTestId('tally-ch13').locator('span')).toHaveCSS('color', 'rgb(255, 255, 255)');
  }
});

test('says in the notice line that changes are not committing while the serving replica has no quorum', { tag: '@mock' }, async ({ page, request }) => {
  test.setTimeout(60_000);
  await live(page);
  const banner = page.getByTestId('commit-banner');
  await expect(banner).toHaveCount(0);
  const logBefore = await page.getByRole('log', { name: 'Notices' }).elementHandle();
  const strip = (await page.locator('[data-channel="ch1"]').boundingBox())!;
  const status = (await page.getByTestId('status').boundingBox())!;
  const primary = (await debug(page, 'primaryReplica'))!;
  for (const r of REPLICAS.filter((x) => x !== primary)) expect((await request.post(`/__mock/replicas/${r}/partition`)).ok()).toBe(true);
  await expect(banner).toBeVisible();
  await expect(banner).toContainText('CHANGES NOT COMMITTING');
  // A status of its own, in no other live region: announced once.
  expect(await banner.evaluate((el) => el.parentElement!.closest('[aria-live], [role="log"], [role="status"], [role="alert"]') === null)).toBe(true);
  // The notices' log stays mounted beside it, empty: the first notice
  // after commits return is announced into a region already there.
  const log = page.getByRole('log', { name: 'Notices' });
  await expect(log).toHaveCount(1);
  await expect(log).toBeEmpty();
  await expect(banner).toContainText('audio continues');
  await expect(page.getByTestId('commit-pending')).toHaveText('0 pending');
  // In the notice line of the status bar, whole and over no control:
  // nothing under it moved and the connection state stays in view.
  const box = (await banner.boundingBox())!;
  const bar = (await page.locator('footer.bar').boundingBox())!;
  expect(box.y).toBeGreaterThanOrEqual(bar.y);
  expect(box.y + box.height).toBeLessThanOrEqual(bar.y + bar.height);
  expect(await banner.locator('.text').evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
  expect(await page.locator('[data-channel="ch1"]').boundingBox()).toEqual(strip);
  expect(await page.getByTestId('status').boundingBox()).toEqual(status);
  await expect(page.getByTestId('quorum')).toBeVisible();
  for (const r of REPLICAS) await request.post(`/__mock/replicas/${r}/restore`);
  await expect(banner).toHaveCount(0);
  // The same log element throughout, never torn down around the banner.
  expect(await logBefore!.evaluate((el) => el.isConnected)).toBe(true);
});

test('moves a processing parameter by the drag distance and never jumps it to the touch point', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch1/gate/threshold', -40);
  await page.locator('[data-channel="ch1"] .head').click();
  const slider = page.locator('[role="slider"][data-path="/channels/ch1/gate/threshold"]');
  await expect(slider).toHaveAttribute('aria-valuetext', '-40 dB');
  const box = (await slider.boundingBox())!;
  // Press at the far right end and let go without moving: nothing changes.
  await page.mouse.move(box.x + box.width - 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.up();
  await page.waitForTimeout(300);
  expect((await serverState(request)).state.channels['ch1']!.gate.threshold).toBe(-40);
  // A drag of a tenth of the width moves the value by a tenth of the range (80 dB).
  await page.mouse.move(box.x + 10, box.y + box.height / 2);
  await page.mouse.down();
  for (let i = 1; i <= 4; i++) await page.mouse.move(box.x + 10 + (box.width / 10) * (i / 4), box.y + box.height / 2);
  await page.mouse.up();
  await settled(page);
  const v = (await serverState(request)).state.channels['ch1']!.gate.threshold;
  expect(v).toBeGreaterThan(-33);
  expect(v).toBeLessThan(-31);
});

test('resets a running loudness measurement only after a one second hold', async ({ page }) => {
  await live(page);
  const resets: string[] = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url().endsWith('/loudness/reset')) resets.push(r.postData() ?? '');
  });
  const key = page.locator('[data-master="pgm20"]').getByRole('button', { name: 'Reset loudness, hold for one second' });
  await key.click();
  await page.waitForTimeout(400);
  expect(resets).toEqual([]);
  const box = (await key.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(1200);
  await page.mouse.up();
  await expect.poll(() => resets.length).toBe(1);
  expect(JSON.parse(resets[0]!)).toEqual({ ids: ['pgm20'] });
});

test('pauses the loudness integration of an output after a hold and runs it on with one press', { tag: '@meters' }, async ({ page, request }) => {
  await live(page);
  const master = page.locator('[data-master="pgm20"]');
  const integrated = page.getByTestId('loudness-i-pgm20');
  await expect(integrated).not.toHaveText(/--\.-/, { timeout: 10_000 });
  const run = master.getByRole('button', { name: 'Loudness integration running, hold to pause' });
  await expect(run).toHaveAttribute('aria-pressed', 'false');
  await expect(run).toHaveText('PAUSE');
  // A tap pauses nothing.
  await run.click();
  await page.waitForTimeout(800);
  expect((await serverState(request)).state.loudness['pgm20']?.running).toBe(true);

  const box = (await run.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect.poll(async () => (await serverState(request)).state.loudness['pgm20']?.running).toBe(false);
  const paused = master.getByRole('button', { name: 'Loudness integration paused, press to run' });
  await expect(paused).toHaveAttribute('aria-pressed', 'true');
  await expect(paused).toHaveText('PAUSED');
  // Lit, unlike a running one, and whole at the master's width.
  expect(await paused.evaluate((e) => getComputedStyle(e).backgroundColor)).not.toBe(await master.getByRole('button', { name: /^Reset loudness/ }).evaluate((e) => getComputedStyle(e).backgroundColor));
  expect(await paused.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
  await expect(integrated).toHaveAttribute('title', /Integration paused/);
  // Paused, the integrated value holds while the programme goes on.
  const held = await integrated.textContent();
  for (let i = 0; i < 4; i++) {
    await page.waitForTimeout(500);
    expect(await integrated.textContent()).toBe(held);
  }
  await paused.click();
  await expect.poll(async () => (await serverState(request)).state.loudness['pgm20']?.running).toBe(true);
  await expect(run).toHaveAttribute('aria-pressed', 'false');
  await expect(integrated).not.toHaveAttribute('title', /.+/);
});

test('keeps replica detail out of the sightline behind one connection lamp', async ({ page }) => {
  await live(page);
  const primary = (await debug(page, 'primaryReplica'))!;
  await expect(page.locator(`[data-replica="${primary}"]`)).toBeHidden();
  await expect(page.getByTestId('meter-replica')).toBeHidden();
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  const details = page.getByRole('region', { name: 'Connection details' });
  await expect(details).toBeVisible();
  await expect(details).toContainText(String(await debug(page, 'revision')).slice(0, 1));
  await expect(page.locator(`[data-replica="${primary}"]`)).toBeVisible();
  await page.getByRole('button', { name: /^Connection LIVE, details$/ }).click();
  await expect(details).toBeHidden();
});

test('keeps a strip to five tab stops and skips to the masters and the monitor', async ({ page, baseURL }) => {
  await live(page);
  // The first tab stop of the page is the skip link, shown when focused.
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'Skip to masters' })).toBeFocused();
  await expect(page.getByRole('link', { name: 'Skip to masters' })).toBeInViewport();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('region', { name: 'Masters' })).toBeFocused();
  await page.getByRole('link', { name: 'Skip to monitor' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('region', { name: 'Monitor' })).toBeFocused();

  // Five controls, then the VCA row of a strip in a VCA: its tooltip
  // names the VCAs and the effective gain, so it stays reachable. Pres 1
  // is in VCA PRES. Another session holds its fader: the holder slot adds
  // no stop.
  const other = await RawClient.open(baseURL!);
  expect((await other.command({ type: 'touch', path: '/channels/ch1/fader' })).type).toBe('delta');
  await expect(page.getByTestId('holder-ch1')).not.toHaveText('');
  await page.locator('[data-channel="ch1"] .head').focus();
  const stops: string[] = [];
  for (let i = 0; i < 12; i++) {
    stops.push(
      await page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null;
        if (!el?.closest('[data-channel="ch1"]')) return 'outside';
        return el.classList.contains('head') ? 'head' : el.classList.contains('vca') ? 'vca' : (el.getAttribute('aria-label') ?? '');
      }),
    );
    await page.keyboard.press('Tab');
  }
  await other.command({ type: 'release', path: '/channels/ch1/fader' });
  other.close();
  expect(stops.filter((s) => s !== 'outside')).toEqual(['head', 'Pres 1 PFL', 'Pres 1 AFL', 'Pres 1 mute', 'Pres 1 fader', 'vca']);
  await expect(page.getByRole('region', { name: 'Channel strips' })).toBeVisible();
});

test('lists the consoles and every other session with what it holds, each once', { tag: '@mock' }, async ({ page, baseURL }) => {
  await live(page);
  const other = await RawClient.open(baseURL!);
  // A control surface that holds a fader is a console row, not a session.
  const surface = await RawClient.open(baseURL!, 'console', CONSOLE_SESSIONS['mcu-1']!);
  try {
    await other.command({ type: 'touch', path: '/channels/ch4/fader' });
    await surface.command({ type: 'touch', path: '/channels/ch5/fader' });
    const key = page.getByRole('button', { name: /^Users \d+(,|$)/ });
    // Two connected surfaces, this screen and one web client.
    await expect(key).toHaveText('USERS  4');
    await key.click();
    const panel = page.getByRole('region', { name: 'Users' });
    await expect(panel).toBeVisible();
    await expect(panel.getByTestId('you')).toContainText('(you)');
    // Named by the tag of its handle, never by its session id.
    await expect(panel.getByTestId('user')).toHaveCount(1);
    await expect(panel.getByTestId('user')).toContainText(`web client ${handleTag(ownerHandle(other.session))}`);
    await expect(panel.getByTestId('user')).toContainText('holds Guest 2 fader');
    await expect(panel).toContainText(handleTag(ownerHandle(await debug(page, 'session'))));
    await expect(panel.getByTestId('console').filter({ hasText: 'MCU Pro' })).toContainText('holds Guest 3 fader');
  } finally {
    await other.command({ type: 'release', path: '/channels/ch4/fader' });
    await surface.command({ type: 'release', path: '/channels/ch5/fader' });
    other.close();
    surface.close();
  }
});

test('lists every session connected to the replica in USERS, until it closes', async ({ page, browser, baseURL }) => {
  // Presence is per replica: both pages on the same primary.
  const ws = baseURL!.replace(/^http/, 'ws');
  const path = BACKEND ? '/' : `/?endpoints=${ws}/__mock/replicas/r1/api/v1/ws,${ws}/__mock/replicas/r2/api/v1/ws`;
  await live(page, path);
  const key = page.getByRole('button', { name: /^Users \d+(,|$)/ });
  await key.click();
  const panel = page.getByRole('region', { name: 'Users' });
  await expect(panel.getByTestId('you')).toContainText(handleTag(ownerHandle(await debug(page, 'session'))));
  // "Users 4" and, where a surface is down, ", 1 surface disconnected", which a session coming and going leaves as it is.
  const [, n, down] = /^Users (\d+)(.*)$/.exec((await key.getAttribute('aria-label'))!)!;
  const count = Number(n);
  const context = await browser.newContext();
  const second = await context.newPage();
  await live(second, path);
  const tag = handleTag(ownerHandle(await debug(second, 'session')));
  const row = panel.getByTestId('user').filter({ hasText: tag });
  // Holding nothing, it is listed for being connected.
  await expect(row).toHaveCount(1);
  await expect(row).toContainText('anonymous');
  await expect(row).toContainText(`operator, web client ${tag}`);
  await expect(key).toHaveAttribute('aria-label', `Users ${count + 1}${down}`);
  await context.close();
  await expect(row).toHaveCount(0);
  await expect(key).toHaveAttribute('aria-label', `Users ${count}${down}`);
});

// The count sits in a box of fixed size (Users.svelte), so a count of
// three digits has to show whole within the key, on a tall screen and on
// a short one.
test('shows a USERS count of three digits whole, on a tall and a short screen', { tag: '@mock' }, async ({ page, baseURL }) => {
  const ws = baseURL!.replace(/^http/, 'ws');
  await live(page, `/?endpoints=${ws}/__mock/replicas/r1/api/v1/ws,${ws}/__mock/replicas/r2/api/v1/ws`);
  const clients = await Promise.all(Array.from({ length: 100 }, () => RawClient.open(`${baseURL}/__mock/replicas/r1`)));
  try {
    const key = page.getByRole('button', { name: /^Users \d+(,|$)/ });
    // The mock's OSC tablet is configured and disconnected.
    await expect(key).toHaveAttribute('aria-label', /^Users 1\d\d, 1 surface disconnected$/);
    const count = key.locator('.fixed');
    // The text, which may run over its box, lies within the key's border.
    const whole = () =>
      count.evaluate((el) => {
        const range = document.createRange();
        range.selectNodeContents(el);
        const text = range.getBoundingClientRect();
        const box = el.closest('button')!.getBoundingClientRect();
        const border = parseFloat(getComputedStyle(el.closest('button')!).borderLeftWidth);
        return text.left >= box.left + border && text.right <= box.right - border;
      });
    await expect(count).toHaveText(/^USERS 1\d\d$/);
    expect(await whole()).toBe(true);
    await page.setViewportSize({ width: 1600, height: 480 });
    await expect(count).toHaveText(/^USR 1\d\d$/);
    expect(await whole()).toBe(true);
  } finally {
    for (const c of clients) c.close();
  }
});

test('steps banks with [ and ], jumps to a channel by name or number, and spills a VCA', async ({ page, request }) => {
  await live(page);
  const scrollLeft = () => page.locator('.scroller').evaluate((el) => el.scrollLeft);
  await page.locator('body').press(']');
  await expect(page.getByRole('button', { name: '9-16' })).toHaveAttribute('aria-pressed', 'true');
  await page.locator('body').press('[');
  await expect(page.getByRole('button', { name: '1-8' })).toHaveAttribute('aria-pressed', 'true');

  const jump = page.getByRole('textbox', { name: /^Jump to a channel/ });
  await jump.fill('amb 1');
  await jump.press('Enter');
  await expect.poll(scrollLeft).toBe(12 * 86);
  await jump.fill('3');
  await jump.press('Enter');
  await expect.poll(scrollLeft).toBe(2 * 86);
  await jump.fill('nothing like this');
  await jump.press('Enter');
  await expect(jump).toHaveClass(/miss/);

  const members = (await serverState(request)).state.vcas['vca2']!.members;
  const name = (await serverState(request)).shape.vcas.find((v) => v.id === 'vca2')!.name;
  await page.getByRole('button', { name: `Spill VCA ${name}: show only its members` }).click();
  await expect(page.getByTestId('spill')).toBeVisible();
  await expect(page.locator('.strips .strip')).toHaveCount(members.length);
  await page.getByTestId('spill').click();
  await expect(page.locator('.strips .strip')).toHaveCount((await serverState(request)).shape.channels.length);
});

test('jumps, steps banks and follows a surface among the strips of a spilled VCA', { tag: '@mock' }, async ({ page, request }) => {
  // Every channel but the first two: positions on the strips are two less
  // than in the shape.
  const channels = (await serverState(request)).shape.channels.map((c) => c.id);
  await patch(request, '/vcas/vca2/members', channels.slice(2));
  try {
    await live(page);
    const scroller = page.locator('.scroller');
    const scrolledTo = (strip: number) =>
      expect
        .poll(() => scroller.evaluate((el, x) => `${el.scrollLeft} of ${Math.min(x, el.scrollWidth - el.clientWidth)}`, strip * 86), { message: `strip ${strip}` })
        .toMatch(/^(\d+) of \1$/);
    await page.getByRole('button', { name: /^Spill VCA .*: show only its members$/ }).nth(1).click();
    await expect(page.locator('.strips .strip')).toHaveCount(channels.length - 2);
    const jump = page.getByRole('textbox', { name: /^Jump to a channel/ });
    // Channel 12 is the tenth strip shown.
    await jump.fill('12');
    await jump.press('Enter');
    await scrolledTo(9);
    await expect(jump).not.toHaveClass(/miss/);
    // Channel 1 is not a member: a miss, and nothing scrolls.
    await jump.fill('1');
    await jump.press('Enter');
    await expect(jump).toHaveClass(/miss/);
    await scrolledTo(9);
    await jump.blur();
    await page.locator('body').press('[');
    await scrolledTo(0);
    await page.locator('body').press(']');
    await scrolledTo(8);
    // The surface banks to channel 17: the first member there is strip 15.
    await page.getByRole('button', { name: 'FOLLOW' }).click();
    expect((await request.post('/__mock/consoles/mcu-1/bank?offset=16')).ok()).toBe(true);
    await scrolledTo(14);
  } finally {
    await request.post('/__mock/consoles/mcu-1/bank?offset=0');
    await patch(request, '/vcas/vca2/members', ['ch3', 'ch4', 'ch5', 'ch6']);
  }
});

test('opens processing over the strips with the masters and monitor reachable, and moves focus in and back', async ({ page }) => {
  await live(page);
  const head = page.locator('[data-channel="ch1"] .head');
  await head.focus();
  await page.keyboard.press('Enter');
  const panel = page.getByRole('region', { name: 'Pres 1 processing' });
  await expect(panel).toBeVisible();
  await expect(panel.getByRole('heading', { name: 'Pres 1' })).toBeFocused();
  // Nothing of the masters or the monitor lies under the panel.
  for (const target of [page.getByRole('slider', { name: 'PGM 5.1 fader' }), page.getByRole('button', { name: 'CUT', exact: true })]) {
    const box = (await target.boundingBox())!;
    const hit = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('.detail') !== null, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
    expect(hit).toBe(false);
  }
  await page.keyboard.press('Escape');
  await expect(panel).toHaveCount(0);
  await expect(head).toBeFocused();
});

test('puts an aux on the channel faders and gives the faders back on EXIT', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch2/sends/aux1/level', -20);
  const faderBefore = await fader(request, 'ch2');
  const strip = (await page.locator('[data-channel="ch2"]').boundingBox())!;
  await page.getByRole('combobox', { name: 'Sends on faders' }).selectOption({ label: 'IFB 1' });
  await expect(page.getByTestId('sends-on-faders')).toBeVisible();
  await expect(page.getByTestId('fader-value-ch2')).toHaveText(formatDb(-20));
  await dragFader(page, '/channels/ch2/sends/aux1/level', -60);
  await settled(page);
  const send = (await serverState(request)).state.channels['ch2']!.sends['aux1']!.level;
  expect(send).toBeGreaterThan(-20);
  expect(await fader(request, 'ch2')).toBe(faderBefore);
  expect(await page.locator('[data-channel="ch2"]').boundingBox()).toEqual(strip);
  await page.getByTestId('sends-on-faders').click();
  await expect(page.getByRole('slider', { name: 'Pres 2 fader' })).toBeVisible();
  await expect(page.getByTestId('fader-value-ch2')).toHaveText(formatDb(faderBefore));
});

const DUCK_HOLD = '/ducks/comm/hold';

/** The Commentary duck tile of the mixer view, opening the masters where they lie over the strips, and the server's group. */
async function duckTile(page: Page, request: APIRequestContext) {
  await live(page);
  const masters = page.getByRole('button', { name: 'MASTERS' });
  if (await masters.isVisible()) await masters.click();
  const tile = page.locator('[data-duck="comm"]');
  return {
    tile,
    key: page.getByTestId('duck-key-comm'),
    lamp: page.getByTestId('duck-lamp-comm'),
    holder: page.getByTestId('duck-holder-comm'),
    duck: async () => (await serverState(request)).state.ducks['comm']!,
    owner: async () => (await serverState(request)).owners[DUCK_HOLD],
  };
}

/**
 * The lamp of a group held or latched in: the mock reports the envelope
 * at full depth, so it reads the reduction of its deepest target, St 1 at
 * 15 dB; the backend runs no engine here, so no meter frame reports one
 * and the lamp reads the state.
 */
const HELD = BACKEND ? 'DUCK HELD' : 'DUCK -15.0';
const LATCHED = BACKEND ? 'DUCK ON' : 'DUCK -15.0';

test('holds a duck group in while DUCK is held, by pointer and by Space, and latches it with a tap', async ({ page, request }) => {
  const { key, lamp, holder, duck, owner } = await duckTile(page, request);
  try {
    await key.hover();
    await page.mouse.down();
    await expect.poll(async () => (await duck()).hold).toBe(true);
    await expect(lamp).toHaveText(HELD);
    await expect(holder).toHaveText('you');
    await expect(page.getByTestId('duck-chip-ch13')).toHaveAccessibleName(/ducked now by Commentary/);
    await page.waitForTimeout(400);
    await page.mouse.up();
    await expect.poll(async () => (await duck()).hold).toBe(false);
    await expect.poll(owner).toBeUndefined();
    expect((await duck()).mode).toBe('auto');
    await expect(lamp).toHaveText('AUTO');
    await expect(holder).toHaveText('');

    await key.focus();
    await page.keyboard.down(' ');
    await expect.poll(async () => (await duck()).hold).toBe(true);
    await page.waitForTimeout(400);
    await page.keyboard.up(' ');
    await expect.poll(async () => (await duck()).hold).toBe(false);
    expect((await duck()).mode).toBe('auto');

    // A tap latches it on, past this screen; the next tap sets it back to auto.
    await key.click();
    await expect.poll(async () => (await duck()).mode).toBe('on');
    await expect.poll(async () => (await duck()).hold).toBe(false);
    await expect(key).toHaveAccessibleName('Commentary duck, latched, Shift+D+1');
    await expect(lamp).toHaveText(LATCHED);
    await key.click();
    await expect.poll(async () => (await duck()).mode).toBe('auto');
    await expect(lamp).toHaveText('AUTO');

    // Shift+D+1 is the same key on the keyboard.
    await page.locator('body').focus();
    await page.keyboard.down('Shift');
    await page.keyboard.down('KeyD');
    await page.keyboard.down('Digit1');
    await expect.poll(async () => (await duck()).hold).toBe(true);
    await page.waitForTimeout(400);
    await page.keyboard.up('Digit1');
    await page.keyboard.up('KeyD');
    await page.keyboard.up('Shift');
    await expect.poll(async () => (await duck()).hold).toBe(false);
    expect((await duck()).mode).toBe('auto');
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
  }
});

test('bypasses a duck group only after a hold on OFF, and a tap on DUCK never takes it out of bypass', async ({ page, request }) => {
  const { tile, key, lamp, duck } = await duckTile(page, request);
  try {
    const off = tile.getByRole('radio', { name: /^Commentary off/ });
    await off.click();
    await page.waitForTimeout(300);
    expect((await duck()).mode).toBe('auto');
    await off.hover();
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();
    await expect.poll(async () => (await duck()).mode).toBe('off');
    await expect(lamp).toHaveText('BYPASS');
    // A hatch as well as the word: bypassed reads from across the desk without colour.
    const hatch = (l: typeof lamp) => l.evaluate((el) => getComputedStyle(el).backgroundImage);
    expect(await hatch(lamp)).toContain('repeating-linear-gradient');
    await key.click();
    await expect(page.getByTestId('notice')).toContainText('Commentary is bypassed: choose AUTO or ON to arm it');
    await page.waitForTimeout(300);
    expect((await duck()).mode).toBe('off');
    await tile.getByRole('radio', { name: /^Commentary auto/ }).click();
    await expect.poll(async () => (await duck()).mode).toBe('auto');
    expect(await hatch(lamp)).toBe('none');
    // Sensitivity from the tile: the threshold moves by a step, with no
    // jump, and its marker in the reserved key meter row moves with it.
    const marker = page.getByTestId('duck-keymeter-comm').locator('.travel');
    const before = await marker.evaluate((el) => getComputedStyle(el).transform);
    const thr = page.getByRole('slider', { name: 'Commentary duck threshold' });
    await thr.focus();
    await page.keyboard.press('ArrowLeft');
    await expect.poll(async () => (await duck()).threshold).toBe(-31);
    await expect(page.getByTestId('duck-threshold-comm')).toHaveText('-31.0');
    await expect.poll(() => marker.evaluate((el) => getComputedStyle(el).transform)).not.toBe(before);
    expect((await page.getByTestId('duck-keymeter-comm').boundingBox())!.height).toBe(8);
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
    await patch(request, '/ducks/comm/threshold', -30);
  }
});

test('a second client sees who holds a duck, cannot take the hold but can latch, and the hold ends with the holder', async ({ page, browser, request }) => {
  const a = await duckTile(page, request);
  const other = await browser.newContext();
  try {
    await a.key.hover();
    await page.mouse.down();
    await expect.poll(async () => (await a.duck()).hold).toBe(true);
    const b = await duckTile(await other.newPage(), request);
    const tag = (await a.owner())!;
    await expect(b.holder).toHaveText(handleTag(tag.handle));
    // The key's face names the holder, read from across the desk.
    await expect(b.key).toHaveText(handleTag(tag.handle));
    await expect(a.key).toHaveText('you');
    await expect(b.lamp).toHaveText(HELD);
    await expect(b.key).toHaveAccessibleName(`Commentary duck, held by web client ${handleTag(tag.handle)}, Shift+D+1`);
    // B's press holds nothing and names the holder; its tap still latches.
    await b.key.click();
    await expect(b.tile.page().getByTestId('notice')).toContainText(`Commentary duck hold is held by web client ${handleTag(tag.handle)}`);
    await expect.poll(async () => (await a.duck()).mode).toBe('on');
    expect((await a.owner())!.handle).toBe(tag.handle);
    // The holder's screen goes away mid-hold: the lease ends the hold.
    await page.close();
    await expect.poll(async () => (await b.duck()).hold, { timeout: 15_000 }).toBe(false);
    await expect.poll(b.owner).toBeUndefined();
    await expect(b.holder).toHaveText('');
    await expect(b.key).toHaveText('DUCK');
    await expect(b.lamp).toHaveText(LATCHED);
  } finally {
    await other.close();
    await patch(request, '/ducks/comm/mode', 'auto');
  }
});

test('the duck tile and strip chips show the key level, the gain reduction and the trigger from the meter frames', { tag: '@mock' }, async ({ page, request }) => {
  // Without automation the mock keys every group in use at a steady -50 dBFS.
  const { lamp, key } = await duckTile(page, request);
  const level = page.getByTestId('duck-key-level-comm');
  const marker = page.getByTestId('duck-keymeter-comm').locator('.marker');
  const target = page.getByTestId('duck-chip-ch13');
  const trigger = page.getByTestId('duck-chip-ch1');
  const scale = () => level.evaluate((el) => new DOMMatrix(getComputedStyle(el).transform).a);
  const right = async () => {
    const b = (await level.boundingBox())!;
    return b.x + b.width;
  };
  try {
    // -50 dBFS on the -60..0 scale of the threshold, short of its marker at -30.
    await expect.poll(scale).toBeCloseTo(10 / 60, 3);
    expect(await right()).toBeLessThan((await marker.boundingBox())!.x);
    await expect(lamp).toHaveText('AUTO');
    await expect(target).toHaveText('DK');
    await expect(trigger).toHaveText('KEY');
    await expect(trigger).not.toHaveClass(/\bon\b/);

    // Held in: the lamp and the target's chip read the reduction, ch13 by
    // the deeper of Commentary's 10 dB and Remote comm's 8 dB.
    await key.hover();
    await page.mouse.down();
    await expect(lamp).toHaveText('DUCK -15.0');
    await expect(target).toHaveText('-10');
    await expect(target).toHaveClass(/\bon\b/);
    await expect(target).toHaveAccessibleName(/reducing now/);
    // Past LATCH_MS: a hold, not a tap that would latch it on.
    await page.waitForTimeout(400);
    await page.mouse.up();
    await expect(lamp).toHaveText('AUTO');
    await expect(target).toHaveText('DK');

    // A threshold below the key: the detector engages in auto, the key
    // level passes the marker, the trigger's chip lights.
    await patch(request, '/ducks/comm/threshold', -55);
    await expect(lamp).toHaveText('DUCK -15.0');
    await expect.poll(async () => (await right()) > (await marker.boundingBox())!.x).toBe(true);
    await expect(trigger).toHaveClass(/\bon\b/);
    await expect(trigger).toHaveAccessibleName(/keying Commentary now/);
    await expect(target).toHaveText('-10');
  } finally {
    await page.mouse.up();
    await patch(request, '/ducks/comm/mode', 'auto');
    await patch(request, '/ducks/comm/threshold', -30);
  }
  await expect(lamp).toHaveText('AUTO');
  await expect(trigger).not.toHaveClass(/\bon\b/);
});

test("a strip's duck chip shows only on a member, is a 24 px target, and opens the Duck section of its panel", async ({ page, request }) => {
  await live(page);
  // In no group: the slot stays empty, with no key to press.
  await expect(page.getByTestId('duck-chip-ch3')).not.toHaveRole('button');
  await expect(page.getByTestId('duck-chip-ch3')).toHaveText('');
  const chip = page.getByTestId('duck-chip-ch13');
  await expect(chip).toHaveText('DK');
  await expect(chip).toHaveAccessibleName('Amb 1 duck, lowered by Commentary 10 dB, Remote comm 8 dB');
  // A trigger's chip, in view: 24 px wide, and its hit area reaches 3 px above and below the 18 px face.
  const key = page.getByTestId('duck-chip-ch1');
  await expect(key).toHaveText('KEY');
  const box = (await key.boundingBox())!;
  expect(box.width).toBeGreaterThanOrEqual(24);
  for (const dy of [-11.5, 11.5]) {
    expect(await page.evaluate(([x, y]) => document.elementFromPoint(x!, y!)?.closest('[data-testid]')?.getAttribute('data-testid'), [box.x + box.width / 2, box.y + box.height / 2 + dy])).toBe('duck-chip-ch1');
  }
  await chip.click();
  const section = page.locator('[data-section="duck"]');
  await expect(section).toBeInViewport();
  const duck = async () => (await serverState(request)).state.ducks['remote']!.channels['ch13']!;
  try {
    await section.getByRole('button', { name: 'Amb 1 is lowered by Remote comm' }).click();
    await expect.poll(async () => (await duck()).target).toBe(false);
    await expect(chip).toHaveAccessibleName('Amb 1 duck, lowered by Commentary 10 dB');
    await section.getByRole('button', { name: 'Amb 1 is lowered by Remote comm' }).click();
    await expect.poll(async () => (await duck()).target).toBe(true);
    const depth = section.getByRole('textbox', { name: 'Depth value, type to set' }).nth(1);
    await depth.fill('18');
    await depth.press('Enter');
    await expect.poll(async () => (await duck()).depth).toBe(18);
    // A target cannot key the group that lowers it: said, not sent.
    await section.getByRole('button', { name: 'Amb 1 triggers Remote comm' }).click({ force: true });
    await expect(page.getByTestId('notice')).toContainText('Amb 1 is a target of Remote comm');
    expect((await duck()).trigger).toBe(false);
    // Joining a group puts the chip in the empty slot.
    await patch(request, '/ducks/remote/channels/ch3/target', true);
    await expect(page.getByTestId('duck-chip-ch3')).toHaveText('DK');
  } finally {
    await patch(request, '/ducks/remote/channels/ch13/depth', 8);
    await patch(request, '/ducks/remote/channels/ch13/target', true);
    await patch(request, '/ducks/remote/channels/ch3/target', false);
  }
});

test('a tap on DUCK of a bypassed group sends nothing to the server', { tag: '@mock' }, async ({ page, request }) => {
  const { key, duck } = await duckTile(page, request);
  try {
    await patch(request, '/ducks/comm/mode', 'off');
    await expect(key).toHaveAccessibleName(/, bypassed/);
    await request.post('/__mock/audit/reset');
    await key.click();
    await expect(page.getByTestId('notice')).toContainText('Commentary is bypassed');
    await page.waitForTimeout(500);
    expect((await audit(request)).entries.filter((e) => e.path === DUCK_HOLD)).toEqual([]);
    expect((await duck()).mode).toBe('off');
    // A deliberate hold still ducks it.
    await key.hover();
    await page.mouse.down();
    await expect.poll(async () => (await duck()).hold).toBe(true);
    await page.mouse.up();
    await expect.poll(async () => (await duck()).hold).toBe(false);
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
  }
});

test('takes one tab stop for the mode keys of a tile, whose arrows move the focus and change nothing', async ({ page, request }) => {
  const { tile, duck } = await duckTile(page, request);
  const stops = await tile.evaluate((t) => [...t.querySelectorAll<HTMLElement>('button, [tabindex]')].filter((e) => e.tabIndex >= 0).map((e) => e.getAttribute('aria-label') ?? e.textContent));
  expect(stops).toEqual([expect.stringMatching(/^Commentary auto/), expect.stringMatching(/^Commentary duck/), 'Commentary duck threshold']);
  const auto = tile.getByRole('radio', { name: /^Commentary auto/ });
  await auto.focus();
  await page.keyboard.press('ArrowRight');
  await expect(tile.getByRole('radio', { name: /^Commentary on/ })).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(tile.getByRole('radio', { name: /^Commentary off/ })).toBeFocused();
  await page.waitForTimeout(300);
  expect((await duck()).mode).toBe('auto');
  try {
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press(' ');
    await expect.poll(async () => (await duck()).mode).toBe('on');
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
  }
  // Shift+D+1 only ever holds: a short chord leaves no latch.
  await page.locator('body').focus();
  await page.keyboard.down('Shift');
  await page.keyboard.down('KeyD');
  await page.keyboard.press('Digit1');
  await page.keyboard.up('KeyD');
  await page.keyboard.up('Shift');
  await page.waitForTimeout(300);
  expect(await duck()).toMatchObject({ mode: 'auto', hold: false });
});

test('keeps the tile band to one row, scrolling within it when more groups than fit', async ({ page, request }) => {
  const { tile } = await duckTile(page, request);
  const band = page.getByRole('group', { name: 'Duck groups', exact: true });
  const one = (await tile.boundingBox())!.height;
  // Six more tiles of the same make, as a shape with eight groups has;
  // with eight the band shows no summary beside them (summaryFits).
  await band.evaluate((b) => {
    for (const c of [...b.children]) if (c.classList.contains('summary')) c.remove();
    b.classList.remove('summary');
    for (let i = 0; i < 6; i++) b.append(b.firstElementChild!.cloneNode(true));
  });
  const m = await band.evaluate((b) => ({ ch: b.clientHeight, sh: b.scrollHeight }));
  expect(m.ch).toBeLessThanOrEqual(Math.ceil(one) + 1);
  expect(m.sh).toBeGreaterThan(m.ch);
});

test('works every duck group from the bank bar with the masters closed at 1280 and 1180 px', async ({ page, request }) => {
  for (const vp of [{ width: 1280, height: 800 }, { width: 1180, height: 820 }]) {
    await page.setViewportSize(vp);
    await live(page);
    await expect(page.getByRole('button', { name: 'MASTERS' })).toHaveAttribute('aria-pressed', 'false');
    const pill = page.getByTestId('duck-pill-comm');
    await expect(pill).toBeInViewport();
    await expect(pill).toHaveText(/Commentary\s*AUTO/);
    await expect(page.getByTestId('duck-pill-remote')).toBeInViewport();
    const duck = async () => (await serverState(request)).state.ducks['comm']!;
    try {
      await pill.click();
      await expect.poll(async () => (await duck()).mode).toBe('on');
      await expect(pill).toHaveText(/Commentary\s*ON/);
      await pill.click();
      await expect.poll(async () => (await duck()).mode).toBe('auto');
      await pill.hover();
      await page.mouse.down();
      await expect.poll(async () => (await duck()).hold).toBe(true);
      await expect(pill).toHaveText(/Commentary\s*HELD/);
      await page.waitForTimeout(400);
      await page.mouse.up();
      await expect.poll(async () => (await duck()).hold).toBe(false);
      expect((await duck()).mode).toBe('auto');
      await patch(request, '/ducks/comm/mode', 'off');
      await expect(pill).toHaveText(/Commentary\s*BYP/);
      expect(await pill.evaluate((el) => getComputedStyle(el).backgroundImage)).toContain('repeating-linear-gradient');
    } finally {
      await patch(request, '/ducks/comm/mode', 'auto');
    }
  }
  // Beside the masters the tiles are in sight, and the bank bar has no pills.
  await page.setViewportSize({ width: 1600, height: 1000 });
  await expect(page.getByTestId('duck-pill-comm')).toBeHidden();
});

test('every state word of a bank-bar pill fits its box at 1024, 1180 and 1280 px in both themes', async ({ page, request }) => {
  await live(page);
  const pill = page.getByTestId('duck-pill-comm');
  const word = pill.locator('.pstate');
  const fits = () => word.evaluate((el) => el.scrollWidth <= el.clientWidth);
  const found: string[] = [];
  try {
    for (const theme of ['dark', 'light']) {
      for (const width of [1024, 1180, 1280]) {
        await page.setViewportSize({ width, height: 820 });
        for (const mode of ['auto', 'on', 'off'] as const) {
          await patch(request, '/ducks/comm/mode', mode);
          await expect(word).toHaveText({ auto: 'AUTO', on: 'ON', off: 'BYP' }[mode]);
          if (!(await fits())) found.push(`${theme} ${width} ${mode}`);
        }
        await patch(request, '/ducks/comm/mode', 'auto');
        await pill.hover();
        await page.mouse.down();
        await expect(word).toHaveText('HELD');
        if (!(await fits())) found.push(`${theme} ${width} held`);
        // Held past the latch time: momentary, no latch left behind.
        await page.waitForTimeout(400);
        await page.mouse.up();
        await expect(word).toHaveText('AUTO');
      }
      if (theme === 'dark') await switchTheme(page, 'light');
    }
  } finally {
    await patch(request, '/ducks/comm/mode', 'auto');
  }
  expect(found).toEqual([]);
});
