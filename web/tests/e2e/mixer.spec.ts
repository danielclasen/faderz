import { expect, test } from '@playwright/test';
import {
  RawClient,
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
  await page.getByRole('button', { name: `Recall ${name}` }).click();
  await expect.poll(() => fader(request, 'ch18')).toBe(stored);
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.getByTestId('fader-value-ch18')).toHaveText(formatDb(stored));
});

test('refuses a fader held by another session with 409 and shows its value and owner', async ({ page, baseURL, request }) => {
  await live(page);
  const path = '/channels/ch4/fader';
  const other = await RawClient.open(baseURL!);
  try {
    expect((await other.command({ type: 'touch', path })).type).toBe('delta');
    const owner = (await serverState(request)).owners[path];
    expect(owner).toMatchObject({ principal: 'anonymous', session: other.session, kind: 'ui' });
    await expect(page.locator('[data-channel="ch4"]').getByTestId('owner')).toHaveAttribute('data-owner-kind', 'ui');
    const held = await fader(request, 'ch4');
    const tag = other.session.replace(/-/g, '').slice(0, 6);
    await dragFader(page, path, -80);
    await expect(page.getByTestId('notice').filter({ hasText: `held by ui ${tag}` }).first()).toBeVisible();
    await settled(page);
    await expect(page.getByTestId('fader-value-ch4')).toHaveText(formatDb(held));
    expect(await fader(request, 'ch4')).toBe(held);
    expect((await other.command({ type: 'release', path })).type).toBe('delta');
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
  await page.keyboard.press('0');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Shift+ArrowDown');
  await settled(page);
  await expect(slider).toHaveAttribute('aria-valuenow', '0.9');
  expect(await fader(request, 'ch21')).toBe(0.9);
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
  await page.getByRole('radio', { name: 'Right to both' }).click();
  await settled(page);
  expect((await serverState(request)).state.channels['st1']!.image).toBe('rightToBoth');
  await expect(page.getByRole('radio', { name: 'Right to both' })).toHaveAttribute('aria-checked', 'true');
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
