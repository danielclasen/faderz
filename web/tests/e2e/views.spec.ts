import { expect, test } from '@playwright/test';
import { live, patch, resetMock, serverState, settled } from './support';

test.beforeEach(async ({ request }) => resetMock(request));

test('renders every view without an error, with live meters on the aux masters', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(m.text());
  });
  await live(page);
  for (const name of ['Routing', 'Aux / N-1', 'AFV', 'Duck', 'Scenes', 'Setup', 'Mixer']) {
    await page.getByRole('button', { name, exact: true }).click();
    await expect(page.getByRole('button', { name, exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(page.getByTestId('view-error')).toHaveCount(0);
    if (name === 'Duck') {
      await expect(page.getByRole('table', { name: 'Duck groups' })).toBeVisible();
      await expect(page.getByRole('switch', { name: 'Pres 1 triggers Commentary' })).toHaveAttribute('aria-checked', 'true');
    }
    if (name === 'Aux / N-1') {
      await expect(page.locator('[data-master="aux1"]')).toBeVisible();
      await expect(page.locator('[data-master="mm1"]')).toBeVisible();
    }
  }
  expect(errors).toEqual([]);
});

test('sets up a duck group in the Duck view: members, refusals, depth, destinations and presets', async ({ page, request }) => {
  const duck = async () => (await serverState(request)).state.ducks['comm']!;
  await live(page);
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  try {
    // A target cannot key the group that lowers it: the screen says why and sends nothing.
    const amb = page.getByRole('switch', { name: 'Amb 1 triggers Commentary' });
    await expect(amb).toHaveAttribute('aria-disabled', 'true');
    // aria-disabled: a pointer still reaches it, which Playwright only does when forced.
    await amb.click({ force: true });
    await expect(page.getByTestId('notice')).toContainText('Amb 1 is a target of Commentary');
    expect((await duck()).channels['ch13']!.trigger).toBe(false);

    const guest = page.getByRole('switch', { name: 'Guest 1 is lowered by Commentary' });
    await guest.click();
    await expect.poll(async () => (await duck()).channels['ch3']!.target).toBe(true);
    const depth = page.getByRole('textbox', { name: /^Guest 1 depth under Commentary/ });
    await depth.fill('12.5');
    await depth.press('Enter');
    await expect.poll(async () => (await duck()).channels['ch3']!.depth).toBe(12.5);

    // The grid is one tab stop; the arrows walk it.
    const first = page.getByRole('switch', { name: 'Pres 1 triggers Commentary' });
    await first.focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('switch', { name: 'Pres 1 is lowered by Commentary' })).toBeFocused();
    await page.keyboard.press('ArrowDown');
    await expect(page.getByRole('switch', { name: 'Pres 2 is lowered by Commentary' })).toBeFocused();
    expect(await page.locator('tbody.members [tabindex="0"]').count()).toBe(1);

    await page.getByRole('button', { name: 'Commentary lowers its targets in M&E' }).click();
    await expect.poll(async () => (await duck()).destinations['me']).toBe(true);

    // HEAVY: one atomic write of the depth of every target and the times.
    await page.getByRole('button', { name: /^Commentary preset Heavy/ }).click();
    await expect.poll(async () => (await duck()).channels['ch3']!.depth).toBe(15);
    const d = await duck();
    expect([d.channels['ch13']!.depth, d.channels['ch1']!.depth, d.attackMs, d.holdMs, d.releaseMs]).toEqual([15, 10, 10, 500, 1500]);
    await page.getByRole('button', { name: /^Commentary voice focus/ }).click();
    await expect.poll(async () => (await duck()).keyLpf).toEqual({ enabled: true, freq: 4000 });
    await settled(page);
  } finally {
    const undo: Array<[string, unknown]> = [
      ['/ducks/comm/channels/ch3/target', false],
      ['/ducks/comm/channels/ch3/depth', 10],
      ['/ducks/comm/destinations/me', false],
      ...['ch13', 'ch14', 'ch15', 'ch16'].map((c): [string, unknown] => [`/ducks/comm/channels/${c}/depth`, 10]),
      ['/ducks/comm/channels/st1/depth', 15],
      ['/ducks/comm/attackMs', 20],
      ['/ducks/comm/holdMs', 500],
      ['/ducks/comm/releaseMs', 1000],
      ['/ducks/comm/keyHpf/freq', 120],
      ['/ducks/comm/keyLpf/enabled', false],
    ];
    for (const [path, value] of undo) await patch(request, path, value);
  }
});
