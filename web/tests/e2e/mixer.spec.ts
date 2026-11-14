import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import WebSocket from 'ws';
import type { StateDocument } from '../../src/lib/contract/types';

const HEADERS = { 'X-Samplez-Request': '1', 'Content-Type': 'application/json' };

async function serverState(request: APIRequestContext): Promise<StateDocument> {
  return (await (await request.get('/api/v1/state')).json()) as StateDocument;
}

async function patch(request: APIRequestContext, path: string, value: unknown) {
  const res = await request.patch('/api/v1/state', {
    headers: HEADERS,
    data: { commandId: crypto.randomUUID(), changes: [{ path, value }] },
  });
  expect(res.ok()).toBe(true);
}

async function live(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('link')).toContainText('LIVE');
}

/** Pending commands drained: every write the UI made has been answered. */
async function settled(page: Page) {
  await expect(page.locator('.pending')).toHaveCount(0);
}

async function servingReplica(page: Page): Promise<string> {
  return (await page.locator('[data-testid="replicas"] .serving').getAttribute('data-replica'))!;
}

async function faderBox(page: Page, path: string) {
  const fader = page.locator(`[role="slider"][data-path="${path}"]`);
  await fader.scrollIntoViewIfNeeded();
  return (await fader.boundingBox())!;
}

async function dragFader(page: Page, path: string, dy: number) {
  const box = await faderBox(page, path);
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  for (let i = 1; i <= 5; i++) await page.mouse.move(x, y + (dy * i) / 5);
  await page.mouse.up();
}

test.afterEach(async ({ request }) => {
  for (const id of ['r1', 'r2', 'r3']) await request.post(`/__mock/replicas/${id}/restore`);
});

test('loads the mixer from a snapshot and streams meters', async ({ page }) => {
  await live(page);
  await expect(page.locator('[data-channel]')).toHaveCount(29);
  await expect(page.locator('[data-master]')).toHaveCount(4);
  await expect(page.getByRole('button', { name: 'Pres 1 mute' })).toBeVisible();
  await expect(page.locator('.meters-from b')).toHaveText(/^r\d$/);
  await expect(page.getByTestId('loudness-i-pgm20')).not.toHaveText('--.-', { timeout: 10_000 });
});

test('moves a fader and shows the value the server echoed', async ({ page, request }) => {
  await live(page);
  const before = (await serverState(request)).state.channels['ch3']!.fader;
  await dragFader(page, '/channels/ch3/fader', -60);
  await settled(page);
  const shown = await page.getByTestId('fader-value-ch3').textContent();
  const after = (await serverState(request)).state.channels['ch3']!.fader;
  expect(after).toBeGreaterThan(before);
  expect(shown).toBe(after > 0 ? `+${after.toFixed(1)}` : after.toFixed(1));
  const owners = (await serverState(request)).owners;
  expect(owners['/channels/ch3/fader']).toBeUndefined();
});

test('applies another client\'s change as a delta', async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch17/fader', -12.5);
  await expect(page.getByTestId('fader-value-ch17')).toHaveText('-12.5');
});

test('mutes a channel', async ({ page, request }) => {
  await live(page);
  const mute = page.getByRole('button', { name: 'Pres 2 mute' });
  await expect(mute).toHaveAttribute('aria-pressed', 'false');
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', 'true');
  await settled(page);
  expect((await serverState(request)).state.channels['ch2']!.mute).toBe(true);
});

test('stores and recalls a scene', async ({ page, request }) => {
  await live(page);
  await page.getByRole('button', { name: 'Scenes' }).click();
  await page.getByLabel('New scene name').fill('E2E scene');
  await page.getByRole('button', { name: 'STORE NEW' }).click();
  await expect(page.getByText('E2E scene')).toBeVisible();
  const stored = (await serverState(request)).state.channels['ch18']!.fader;
  await patch(request, '/channels/ch18/fader', -33);
  await page.getByRole('button', { name: 'Recall E2E scene' }).click();
  await expect.poll(async () => (await serverState(request)).state.channels['ch18']!.fader).toBe(stored);
  await page.getByRole('button', { name: 'Mixer' }).click();
  await expect(page.getByTestId('fader-value-ch18')).toHaveText(stored.toFixed(1));
});

test('refuses a fader held by another client with 409 and shows its value', async ({ page, baseURL, request }) => {
  await live(page);
  const path = '/channels/ch4/fader';
  const other = new WebSocket(`${baseURL!.replace('http', 'ws')}/api/v1/ws`);
  await new Promise((r) => other.once('open', r));
  other.send(JSON.stringify({ type: 'hello', clientId: 'e2e-console' }));
  other.send(JSON.stringify({ type: 'touch', id: crypto.randomUUID(), path }));
  await expect.poll(async () => (await serverState(request)).owners[path]).toBe('e2e-console');
  const held = (await serverState(request)).state.channels['ch4']!.fader;
  await dragFader(page, path, -80);
  await expect(page.getByTestId('notice').filter({ hasText: 'held by e2e-console' }).first()).toBeVisible();
  await settled(page);
  await expect(page.getByTestId('fader-value-ch4')).toHaveText(held.toFixed(1));
  expect((await serverState(request)).state.channels['ch4']!.fader).toBe(held);
  other.close();
});

test('reconnects and resyncs after a dropped link and after a revision gap', async ({ page, request }) => {
  await live(page);
  await request.post('/__mock/drop-clients');
  await expect(page.getByTestId('link')).toContainText('LIVE');
  await request.post('/__mock/skip-revision');
  await patch(request, '/channels/ch19/fader', -7);
  await expect(page.getByTestId('fader-value-ch19')).toHaveText('-7.0');
  const server = await serverState(request);
  await expect(page.getByTestId('revision')).toHaveText(`rev ${server.revision}`);
});

for (const mode of ['drop', 'apply'] as const) {
  test(`fails over from a hung replica (${mode}) without losing or duplicating commands`, async ({ page, request }) => {
    await live(page);
    const from = await servingReplica(page);
    await request.post(`/__mock/replicas/${from}/freeze?mode=${mode}`);
    const channel = mode === 'drop' ? { id: 'ch7', name: 'Rpt 1' } : { id: 'ch8', name: 'Rpt 2' };
    const mute = page.getByRole('button', { name: `${channel.name} mute` });
    await mute.click();
    await mute.click();
    await mute.click();
    await expect(mute).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByTestId('failover')).toBeVisible({ timeout: 8000 });
    await expect(page.getByTestId('link')).toContainText('LIVE');
    await settled(page);
    expect(await servingReplica(page)).not.toBe(from);
    expect((await serverState(request)).state.channels[channel.id]!.mute).toBe(true);
    const audit = (await (await request.get('/__mock/commands')).json()) as Array<{ commandId: string; path: string | null; result: string }>;
    const mine = audit.filter((a) => a.path === `/channels/${channel.id}/mute`);
    const applied = mine.filter((a) => a.result === 'applied');
    expect(applied).toHaveLength(3);
    expect(new Set(applied.map((a) => a.commandId)).size).toBe(3);
    expect(mine.every((a) => a.result === 'applied' || a.result === 'duplicate')).toBe(true);
  });
}

test('keeps a held fader owned across a replica kill', async ({ page, request }) => {
  await live(page);
  const path = '/channels/ch20/fader';
  const from = await servingReplica(page);
  const box = await faderBox(page, path);
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y - 20);
  await expect.poll(async () => (await serverState(request)).owners[path]).toBeTruthy();
  const owner = (await serverState(request)).owners[path];
  await request.post(`/__mock/replicas/${from}/kill`);
  await expect(page.getByTestId('failover')).toBeVisible({ timeout: 8000 });
  await page.mouse.move(x, y - 60);
  await expect(page.locator(`[data-replica="${from}"]`)).toHaveClass(/down/);
  await page.waitForTimeout(2500);
  expect((await serverState(request)).owners[path]).toBe(owner);
  await page.mouse.up();
  await settled(page);
  await expect.poll(async () => (await serverState(request)).owners[path]).toBeUndefined();
  const value = (await serverState(request)).state.channels['ch20']!.fader;
  await expect(page.getByTestId('fader-value-ch20')).toHaveText(value > 0 ? `+${value.toFixed(1)}` : value.toFixed(1));
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
  const fader = page.locator('[role="slider"][data-path="/channels/ch21/fader"]');
  await fader.focus();
  await page.keyboard.press('0');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Shift+ArrowDown');
  await settled(page);
  await expect(fader).toHaveAttribute('aria-valuenow', '0.9');
  expect((await serverState(request)).state.channels['ch21']!.fader).toBe(0.9);
});
