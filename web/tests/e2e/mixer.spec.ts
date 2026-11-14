import { expect, test } from '@playwright/test';
import { handleTag, ownerHandle } from '../../src/lib/net/handle';
import {
  RawClient,
  audit,
  debug,
  dragFader,
  fader,
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

test('refuses a fader held by another session with 409 and shows its value and owner', async ({ page, baseURL, request }) => {
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

test('switches to the light theme and works at tablet width', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await live(page);
  await page.getByRole('button', { name: 'Switch to light theme' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  const width = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(width).toBeLessThanOrEqual(1024);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
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
  const talk = page.getByRole('button', { name: 'N-1 Phone 1 talk, hold' });
  await talk.hover();
  await page.mouse.down();
  await expect.poll(async () => (await serverState(request)).state.mixMinus['mm1']!.interrupt.active).toBe(true);
  await expect(talk).toHaveAttribute('aria-pressed', 'true');
  await page.mouse.up();
  await expect.poll(async () => (await serverState(request)).state.mixMinus['mm1']!.interrupt.active).toBe(false);
  await expect.poll(async () => (await serverState(request)).owners['/mixMinus/mm1/interrupt/active']).toBeUndefined();
  await patch(request, '/mixMinus/mm1/interrupt/source', null);
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
      return { text: s.textContent, centre: r.x + r.width / 2 };
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
  });
});

test('latches a true-peak over of a channel direct out until cleared', async ({ page, request }) => {
  await live(page);
  const overs = page.getByTestId('channel-overs');
  await expect(overs).toBeDisabled();
  await patch(request, '/channels/ch1/trim', 20);
  await expect(overs).toBeEnabled();
  await expect(overs).toHaveAccessibleName(/Pres 1/);
  await patch(request, '/channels/ch1/trim', 0);
  await page.waitForTimeout(300);
  await expect(overs).toHaveAccessibleName(/Pres 1/);
  await overs.click();
  await expect(overs).toBeDisabled();
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
