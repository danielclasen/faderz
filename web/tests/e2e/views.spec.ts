import { expect, test, type APIRequestContext } from '@playwright/test';
import { live, patch, patchAll, resetMock, serverState, settled } from './support';

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

    // HEAVY rewrites every target's depth and the times: one press arms
    // it in place, leaving it disarms it, and a second press applies it.
    const heavy = page.locator('[aria-label^="Commentary preset heavy"], [aria-label^="Confirm Commentary preset heavy"]');
    await heavy.click();
    await expect(heavy).toHaveText('APPLY');
    await expect(heavy).toHaveAccessibleName(/^Confirm Commentary preset heavy/);
    await page.waitForTimeout(400);
    expect((await duck()).attackMs).toBe(20);
    await page.keyboard.press('Tab');
    await expect(heavy).toHaveText('HEAVY');
    await heavy.click();
    await heavy.click();
    await expect.poll(async () => (await duck()).channels['ch3']!.depth).toBe(15);
    const d = await duck();
    expect([d.channels['ch13']!.depth, d.channels['ch1']!.depth, d.attackMs, d.holdMs, d.releaseMs]).toEqual([15, 10, 10, 500, 1500]);
    await expect(heavy).toHaveText('HEAVY');
    // A hold of 0.5 s applies a preset at once.
    const voice = page.getByRole('button', { name: /^Commentary preset voice focus/ });
    await voice.hover();
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();
    await expect.poll(async () => (await duck()).keyLpf).toEqual({ enabled: true, freq: 4000 });
    await expect(voice).toHaveText('VOICE');
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

test('folds the detector up to 1280 px, so the channel rows are in view, and unfolds it on request', async ({ page }) => {
  await page.setViewportSize({ width: 1180, height: 820 });
  await live(page);
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  await expect(page.getByRole('switch', { name: 'Pres 1 triggers Commentary' })).toBeInViewport();
  await expect(page.getByRole('switch', { name: 'Amb 1 is lowered by Commentary' })).toBeInViewport();
  await expect(page.getByRole('slider', { name: 'Hyst' })).toHaveCount(0);
  // The mode keys stay in reach, folded.
  await expect(page.getByRole('radiogroup', { name: 'Commentary duck mode' })).toBeVisible();
  const fold = page.getByRole('button', { name: 'Detector settings' });
  await expect(fold).toHaveAttribute('aria-expanded', 'false');
  await fold.click();
  await expect(fold).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByRole('slider', { name: 'Hyst' }).first()).toBeVisible();
  // Wider, it starts unfolded.
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.reload();
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  await expect(page.getByRole('slider', { name: 'Hyst' }).first()).toBeVisible();
});

/** The Commentary group as the preset test found it, in one write. */
const restoreComm = (request: APIRequestContext) =>
  patchAll(request, [
    ...['ch13', 'ch14', 'ch15', 'ch16'].map((c) => ({ path: `/ducks/comm/channels/${c}/depth`, value: 10 })),
    { path: '/ducks/comm/channels/st1/depth', value: 15 },
    { path: '/ducks/comm/attackMs', value: 20 },
    { path: '/ducks/comm/holdMs', value: 500 },
    { path: '/ducks/comm/releaseMs', value: 1000 },
    { path: '/ducks/comm/keyHpf/freq', value: 120 },
    { path: '/ducks/comm/keyLpf/enabled', value: false },
  ]);

test('arms a preset from the keyboard, applies it on a second press, and a keyboard hold swallows no later click', async ({ page, request }) => {
  const duck = async () => (await serverState(request)).state.ducks['comm']!;
  await live(page);
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  const heavy = page.locator('[aria-label^="Commentary preset heavy"], [aria-label^="Confirm Commentary preset heavy"]');
  const voice = page.locator('[aria-label^="Commentary preset voice"], [aria-label^="Confirm Commentary preset voice"]');
  try {
    await heavy.focus();
    await page.keyboard.press('Enter');
    await expect(heavy).toHaveText('APPLY');
    await page.waitForTimeout(300);
    expect((await duck()).attackMs).toBe(20);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await duck()).attackMs).toBe(10);
    await expect(heavy).toHaveText('HEAVY');
    // A keyboard hold applies at once; the next mouse press on any preset still counts.
    await page.keyboard.down('Space');
    await page.waitForTimeout(700);
    await page.keyboard.up('Space');
    await page.waitForTimeout(300);
    await expect(heavy).toHaveText('HEAVY');
    await voice.click();
    await expect(voice).toHaveText('APPLY');
    await voice.click();
    await expect.poll(async () => (await duck()).keyLpf).toEqual({ enabled: true, freq: 4000 });
  } finally {
    await restoreComm(request);
  }
});

test('gives a mouse depth fields and the tile threshold at least 24 px', async ({ page }) => {
  await live(page);
  expect((await page.getByRole('slider', { name: 'Commentary duck threshold' }).boundingBox())!.height).toBeGreaterThanOrEqual(24);
  await page.getByTestId('duck-chip-ch1').click();
  const section = page.locator('[data-section="duck"]');
  expect((await section.getByRole('textbox', { name: 'Depth value, type to set' }).first().boundingBox())!.height).toBeGreaterThanOrEqual(24);
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  expect((await page.getByRole('textbox', { name: /^Pres 1 depth under Commentary/ }).boundingBox())!.height).toBeGreaterThanOrEqual(24);
});

test('holds a duck group with Shift+D+n in a view without its tile', async ({ page, request }) => {
  const hold = async () => (await serverState(request)).state.ducks['remote']!.hold;
  await live(page);
  for (const name of ['Duck', 'Scenes']) {
    await page.getByRole('button', { name, exact: true }).click();
    await page.locator('body').focus();
    await page.keyboard.down('Shift');
    await page.keyboard.down('KeyD');
    await page.keyboard.down('Digit2');
    await expect.poll(hold).toBe(true);
    await page.waitForTimeout(400);
    await page.keyboard.up('Digit2');
    await page.keyboard.up('KeyD');
    await page.keyboard.up('Shift');
    await expect.poll(hold).toBe(false);
    expect((await serverState(request)).state.ducks['remote']!.mode).toBe('auto');
  }
});

test('keeps one tab stop in the Duck view grid when the groups shown change', async ({ page }) => {
  await live(page);
  await page.getByRole('button', { name: 'Duck', exact: true }).click();
  // The tab stop on the second group's column ...
  await page.getByRole('switch', { name: 'Pres 1 triggers Remote comm' }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('switch', { name: 'Pres 1 is lowered by Remote comm' })).toBeFocused();
  // ... and a phone shows only the first group.
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByRole('combobox', { name: 'Duck group' })).toBeVisible();
  await expect(page.locator('tbody.members [tabindex="0"]')).toHaveCount(1);
  await page.getByRole('combobox', { name: 'Duck group' }).focus();
  await page.keyboard.press('Tab');
  // Through the detector and destination keys, Tab reaches the grid.
  for (let i = 0; i < 40 && !(await page.evaluate(() => !!document.activeElement?.closest('tbody.members'))); i++) await page.keyboard.press('Tab');
  expect(await page.evaluate(() => !!document.activeElement?.closest('tbody.members'))).toBe(true);
});

/** "key -18.3 >= thr -30.0": both numbers named, and the comparison the reading states holds for them. */
function keyingHolds(text: string): boolean {
  const m = /^\s*key\s+(-inf|--|-?\d+\.\d)\s+(>=|<)?\s*thr\s+(-?\d+\.\d)\s*$/.exec(text);
  if (!m) return false;
  if (m[1] === '--') return m[2] === undefined;
  const key = m[1] === '-inf' ? -Infinity : Number(m[1]);
  return (key >= Number(m[3])) === (m[2] === '>=');
}

test('each duck group says what keys it at which tap, what it ducks on which outputs, and its key level against the threshold', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const band = page.getByTestId('duck-summary');
  await expect(band).toBeVisible();
  // The triggers are named as the key, never as what the group lowers.
  await expect(page.getByTestId('duck-summary-keys-comm')).toHaveText('key: Pres 1, Pres 2 (pre-fader, while open)');
  await expect(page.getByTestId('duck-summary-ducks-comm')).toHaveText(/^ducks: Amb 1, .* on PGM 5\.1, PGM 2\.0$/);
  await expect(page.getByTestId('duck-summary-ducks-comm')).not.toContainText('Pres 1');
  await expect(page.getByTestId('duck-summary-keys-remote')).toHaveText(/^key: Phone 1, Phone 2 \(/);
  const keying = page.getByTestId('duck-summary-keying-comm');
  await expect(keying).toHaveText(/-30\.0\s*$/);
  // A reading arrives with the meter frames, its comparison true to its numbers.
  await expect.poll(async () => keyingHolds((await keying.textContent()) ?? ''), { timeout: 5000 }).toBe(true);
  await expect(keying).not.toHaveText(/^\s*key\s+--/, { timeout: 5000 });
  // A click shows the line's tip with every list whole, where the line cuts them.
  await page.getByTestId('duck-summary-comm').click();
  await expect(page.getByRole('tooltip')).toContainText('ducks: Amb 1, Amb 2, Amb 3, Amb 4, Music on PGM 5.1, PGM 2.0');
  await expect(page.getByRole('tooltip')).toContainText('key: Pres 1, Pres 2 (pre-fader, while open)');
  await page.mouse.move(0, 0);
  await expect(page.getByRole('tooltip')).toBeHidden();
  // A changed tap, trigger and threshold read at once, in the band and in the Duck view.
  try {
    await patch(request, '/ducks/comm/keyTap', 'postFader');
    await patch(request, '/ducks/comm/channels/ch2/trigger', false);
    await patch(request, '/ducks/comm/threshold', -24);
    await expect(page.getByTestId('duck-summary-keys-comm')).toHaveText('key: Pres 1 (post-fader)');
    await expect(keying).toHaveText(/-24\.0\s*$/);
    await page.getByRole('button', { name: 'Duck', exact: true }).click();
    await expect(page.getByTestId('duck-view-keys-comm')).toHaveText('key: Pres 1 (post-fader)');
    const ducks = page.getByTestId('duck-view-ducks-comm');
    await expect(ducks).toHaveText(/^ducks: Amb 1, .* on PGM 5\.1, PGM 2\.0$/);
    // The outputs show whole in the 212 px column; the targets give way.
    // The list wraps: the whole of it shows in the 212 px column, nothing cut.
    await expect(ducks).toHaveText('ducks: Amb 1, Amb 2, Amb 3, Amb 4, Music on PGM 5.1, PGM 2.0');
    expect(await ducks.evaluate((e) => e.scrollHeight <= e.clientHeight + 0.5)).toBe(true);
    const level = page.getByTestId('duck-view-keying-comm');
    await expect(level).toHaveText(/^now: key .* thr -24\.0$/);
    await expect.poll(async () => keyingHolds(((await level.locator('.reading').textContent()) ?? '')), { timeout: 5000 }).toBe(true);
  } finally {
    await patch(request, '/ducks/comm/keyTap', 'preFaderOpen');
    await patch(request, '/ducks/comm/channels/ch2/trigger', true);
    await patch(request, '/ducks/comm/threshold', -30);
  }
});

test('names each bus with its width on its master and in the routing columns, as a channel strip does', async ({ page, request }) => {
  const { shape } = await serverState(request);
  const wide = shape.buses.find((b) => b.layout === '5.1' || b.layout === '7.1');
  const stereo = shape.buses.find((b) => b.layout === 'stereo');
  expect(wide, 'the example shape has a bus wider than stereo').toBeDefined();
  await live(page);
  // The master's head: the same badge a 5.1 channel strip shows.
  await expect(page.getByTestId(`layout-${wide!.id}`)).toHaveText(wide!.layout);
  await expect(page.getByTestId(`layout-${stereo!.id}`)).toHaveText('ST');
  // The channel strip's grey width badge, on the master as on a strip.
  for (const badge of [page.getByTestId(`layout-${wide!.id}`).locator('.badge'), page.locator('[data-channel] .head .badge[data-layout]').first()]) {
    await expect(badge).toHaveCSS('background-color', 'rgb(139, 150, 164)');
    await expect(badge).toHaveCSS('color', 'rgb(11, 13, 16)');
    await expect(badge).toHaveCSS('font-size', '10px');
  }
  // Line-up tone takes the width's place while it replaces the programme, moving nothing.
  const box = await page.getByTestId(`layout-${stereo!.id}`).boundingBox();
  try {
    await patch(request, '/tone/targets', [stereo!.id]);
    await patch(request, '/tone/on', true);
    await expect(page.getByTestId(`tone-${stereo!.id}`)).toBeVisible();
    await expect(page.getByTestId(`layout-${stereo!.id}`)).toBeHidden();
    expect(await page.getByTestId(`layout-${stereo!.id}`).boundingBox()).toEqual(box);
  } finally {
    await patch(request, '/tone/on', false);
    await patch(request, '/tone/targets', []);
  }
  await expect(page.getByTestId(`layout-${stereo!.id}`)).toBeVisible();
  // A kind tag the name already says is left out: PGM on "PGM 5.1"; CF on "Clean" stays.
  await expect(page.locator(`[data-master="${wide!.id}"] .tags > .tag`)).toBeHidden();
  const clean = shape.buses.find((b) => b.kind === 'cleanFeed')!;
  await expect(page.locator(`[data-master="${clean.id}"] .tags > .tag`)).toHaveText('CF');
  // A mono output shows none.
  await page.getByRole('button', { name: 'Aux / N-1', exact: true }).click();
  const mono = shape.auxes.find((a) => a.layout === 'mono')!;
  await expect(page.getByTestId(`layout-${mono.id}`).locator('.badge')).toHaveCount(0);
  // Routing: each bus column names its width beside its name; VCA columns have none.
  await page.getByRole('button', { name: 'Routing', exact: true }).click();
  const head = page.getByRole('table', { name: 'Routing' }).locator('thead tr').last();
  await expect(head.getByRole('columnheader', { name: `${wide!.name}, ${wide!.layout}`, exact: true })).toBeVisible();
  await expect(head.getByRole('columnheader', { name: `${stereo!.name}, stereo`, exact: true })).toBeVisible();
  await expect(head.locator('.badge[data-layout]')).toHaveCount(shape.buses.filter((b) => b.layout !== 'mono').length);
  // The output flows name widths as the badges do, not as "stereo".
  const flows = page.locator('section.outputs tbody');
  await expect(flows).not.toContainText('stereo');
  await expect(flows.locator('.badge[data-layout="stereo"]').first()).toHaveText('ST');
});
