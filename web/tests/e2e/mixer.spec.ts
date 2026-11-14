import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { handleTag, ownerHandle } from '../../src/lib/net/handle';
import { engineLamp } from '../../src/lib/counters';
import {
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
  uiFontLoaded,
} from './support';

test.beforeEach(async ({ request }) => resetMock(request));
test.afterEach(async ({ request }) => resetMock(request));

test('loads the mixer from a snapshot and streams meters', async ({ page }) => {
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

test('gives each meter a level for assistive technology and shows the channel meter tap', async ({ page, request }) => {
  await live(page);
  const meter = page.getByRole('meter', { name: /^Pres 1 meter, pre-fader/ });
  await expect(meter).toHaveAttribute('aria-valuetext', /^peak -?\d+ dBFS/, { timeout: 5000 });
  await expect(page.getByTestId('tap-ch1')).toHaveText(' PRE');
  await patch(request, '/channels/ch1/meterTap', 'postFader');
  await expect(page.getByTestId('tap-ch1')).toHaveText('POST');
  await patch(request, '/channels/ch1/meterTap', 'preFader');
});

test("a meter's level for assistive technology ends where its bars do when the meters stop", async ({ page, request }) => {
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

test('refuses a fader held by another session at the gesture and shows its value and owner', async ({ page, baseURL, request }) => {
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
    await expect(page.getByTestId('holder-ch4')).toHaveText(`web client ${tag}`);
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
  await page.getByRole('button', { name: 'Switch to light theme' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
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

test('operates a pan from the keyboard, with no jump from a lone key or a double-click', async ({ page, request }) => {
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
  await page.getByRole('radio', { name: 'Right to both', exact: true }).click();
  await settled(page);
  expect((await serverState(request)).state.channels['st1']!.image).toBe('rightToBoth');
  await expect(page.getByRole('radio', { name: 'Right to both', exact: true })).toHaveAttribute('aria-checked', 'true');
  await patch(request, '/channels/st1/image', 'normal');
  await page.locator('[data-channel="ch1"] .head').click();
  await expect(page.getByLabel('LFE', { exact: true })).toBeVisible();
  await patch(request, '/channels/ch1/pan/lfe', -12);
  await expect(page.locator('input[data-path="/channels/ch1/pan/lfe"]')).toHaveAttribute('aria-valuetext', '-12.0');
  // On the fader taper, as the Aux view's send levels: -12 dB sits at 47.5 % of the travel.
  await expect(page.locator('input[data-path="/channels/ch1/pan/lfe"]')).toHaveValue('475');
  await patch(request, '/channels/ch1/pan/lfe', -90);
});

test('enters a channel delay in frames and shows it in ms', async ({ page, request }) => {
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

test('ends a latched talk when the page is hidden', async ({ page, request }) => {
  // A mouse hold over a tab switch gets no blur on Safari. Playwright
  // cannot hide a headless page or fire a real pagehide without leaving
  // it, so the page reports itself hidden and fires visibilitychange.
  const { talk, active } = await talkKey(page, request);
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  expect(await active()).toBe(true);
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect.poll(active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners[TALK_OWNER]).toBeUndefined();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk');
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
});

test('ends a latched talk when its view is left, so another client can talk', async ({ page, browser, request }) => {
  const { talk, active } = await talkKey(page, request);
  await talk.click();
  await expect(talk).toHaveAccessibleName('N-1 Phone 1 talk, latched');
  expect(await active()).toBe(true);
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

test('a latched talk stopped while writes are locked never shows as unlatched while it is still on', async ({ page, request }) => {
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
  await expect(page.getByTestId('quorum')).toHaveText('NO Q');
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

test('reads loudness in LU relative to the target on request', async ({ page, request }) => {
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

test('latches a true-peak over of a channel direct out until cleared', async ({ page, request }) => {
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

test('latches a true-peak over on a master until RESET, however short it was', async ({ page, request }) => {
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
  await page.locator('[data-master="pgm20"]').getByRole('button', { name: 'RESET' }).click();
  await expect(lamp).not.toHaveClass(/over/);
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('shows a channel true peak only on channels with a direct out', async ({ page, request }) => {
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

test('reads the highest true peak since RESET, not the last tenth of a second', async ({ page, request }) => {
  await live(page);
  const tp = page.getByTestId('tp-pgm20');
  await page.locator('[data-master="pgm20"]').getByRole('button', { name: 'RESET' }).click();
  await patch(request, '/tone/level', 0);
  await patch(request, '/tone/targets', ['pgm20']);
  await patch(request, '/tone/on', true);
  await page.waitForTimeout(150);
  await patch(request, '/tone/on', false);
  await expect(tp).toHaveText(/^\s*-?0\.\d$/);
  await page.waitForTimeout(1500);
  await expect(tp).toHaveText(/^\s*-?0\.\d$/);
  await page.locator('[data-master="pgm20"]').getByRole('button', { name: 'RESET' }).click();
  await expect(tp).not.toHaveText(/^\s*-?0\.\d$/);
  await patch(request, '/tone/level', -18);
  await patch(request, '/tone/targets', []);
});

test('a RESET on one screen clears the true-peak over on every screen', async ({ page, browser, request }) => {
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
  await page.locator('[data-master="pgm20"]').getByRole('button', { name: 'RESET' }).click();
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
  for (const field of [page.getByRole('textbox', { name: 'Thr value, type to set' }).first(), panel.getByRole('textbox', { name: /delay in/i })]) {
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
  // A press shorter than the hold does nothing.
  await lock.click();
  await expect(lock).toHaveAttribute('aria-pressed', 'false');
  const box = (await lock.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect(lock).toHaveAttribute('aria-pressed', 'true');
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
  await expect(page.getByTestId('iso-ch3')).toHaveAttribute('title', 'Recall safe: fader, mute');
  await patch(request, '/recallSafe/ch3', []);
  await expect(page.getByTestId('iso-ch3')).toBeHidden();
});

test('labels the channel meter in dBFS on its outer side, apart from the fader scale', async ({ page }) => {
  await page.addInitScript(() => {
    const seen: Array<{ text: string; x: number; y: number }> = [];
    (window as unknown as { __labels: typeof seen }).__labels = seen;
    const fill = CanvasRenderingContext2D.prototype.fillText;
    CanvasRenderingContext2D.prototype.fillText = function (text: string, x: number, y: number, max?: number) {
      const m = this.getTransform();
      seen.push({ text, x: x * m.a / devicePixelRatio, y: y * m.d / devicePixelRatio });
      return max === undefined ? fill.call(this, text, x, y) : fill.call(this, text, x, y, max);
    };
  });
  await live(page);
  const meter = (await page.locator('[data-meter="channels/ch1"]').boundingBox())!;
  const canvas = (await page.locator('.mixer > .meter-layer').boundingBox())!;
  await expect
    .poll(() =>
      page.evaluate(
        ({ m, c }) =>
          [...new Set((window as unknown as { __labels: Array<{ text: string; x: number; y: number }> }).__labels
            .filter((l) => l.x + c.x >= m.x + m.width - 17 && l.x + c.x <= m.x + m.width && l.y + c.y >= m.y && l.y + c.y <= m.y + m.height)
            .map((l) => l.text))].sort(),
        { m: meter, c: canvas },
      ),
    )
    .toEqual(['-18', '-40', '-9', '0']);
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

test('shows one engine lamp in the status bar and the counters only on request', async ({ page }) => {
  await live(page);
  const lamp = page.getByTestId('engine');
  await expect(lamp).toHaveText(/ENGINE (OK|WARN \d+)/);
  const width = (await lamp.boundingBox())!.width;
  await expect(page.locator('footer.bar')).not.toContainText('deadline_miss');
  await lamp.click();
  const counters = page.getByRole('region', { name: 'Engine counters' });
  // Every total is listed in the panel: none is cut.
  for (const name of ['deadline_miss', 'overrun_skipped_samples', 'input_late_blocks', 'input_wait_timeout', 'command_late', 'afv_late']) {
    await expect(counters).toContainText(name);
  }
  await lamp.click();
  await expect(counters).toHaveCount(0);
  expect((await lamp.boundingBox())!.width).toBe(width);
  // OK and WARN with any count, two digits and capped ones included, give
  // the lamp one width and fit its text: a change moves nothing.
  const sizes = await lamp.evaluate((b, texts) => {
    const text = b.querySelector<HTMLElement>('.fixed')!;
    const was = text.textContent;
    const out = texts.map((t) => {
      text.textContent = t;
      return { w: b.getBoundingClientRect().width, fits: text.scrollWidth <= text.getBoundingClientRect().width + 0.5 };
    });
    text.textContent = was;
    return out;
  }, [0, 3, 12, 99, 250].map(engineLamp));
  for (const s of sizes) expect(s).toEqual({ w: width, fits: true });
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
});

test('says in the header that changes are not committing while the serving replica has no quorum', async ({ page, request }) => {
  test.setTimeout(60_000);
  await live(page);
  const banner = page.getByTestId('commit-banner');
  await expect(banner).toHaveCount(0);
  const strip = (await page.locator('[data-channel="ch1"]').boundingBox())!;
  const primary = (await debug(page, 'primaryReplica'))!;
  for (const r of REPLICAS.filter((x) => x !== primary)) expect((await request.post(`/__mock/replicas/${r}/partition`)).ok()).toBe(true);
  await expect(banner).toBeVisible();
  await expect(banner).toContainText('CHANGES NOT COMMITTING');
  await expect(banner).toContainText('audio continues');
  // In the header: nothing under it moved, and the controls stay reachable.
  expect((await banner.boundingBox())!.y).toBeLessThan(strip.y);
  expect(await page.locator('[data-channel="ch1"]').boundingBox()).toEqual(strip);
  for (const r of REPLICAS) await request.post(`/__mock/replicas/${r}/restore`);
  await expect(banner).toHaveCount(0);
});
