import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { DEAD_MS } from '../../src/lib/contract/protocol';
import { ownerHandle } from '../../src/lib/net/handle';
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
  standbyReady,
  stats,
} from './support';

test.beforeEach(async ({ request }) => resetMock(request));
test.afterEach(async ({ request }) => resetMock(request));

/** Records the page time at which the primary is a replica other than `from` and the link is Live. */
async function watchSwitch(page: Page, from: string): Promise<void> {
  await page.evaluate((from) => {
    const w = window as unknown as { __switchedAt?: number };
    delete w.__switchedAt;
    const poll = () => {
      const f = window.__faderz!;
      if (f.primaryReplica !== null && f.primaryReplica !== from && f.connection === 'Live') w.__switchedAt = Date.now();
      else setTimeout(poll, 1);
    };
    poll();
  }, from);
}

async function switchedAt(page: Page): Promise<number> {
  await page.waitForFunction(() => (window as unknown as { __switchedAt?: number }).__switchedAt !== undefined);
  return page.evaluate(() => (window as unknown as { __switchedAt: number }).__switchedAt);
}

/**
 * Every set of `session` on `path` that reached a replica was applied
 * exactly once, in seq order, and resends were answered as duplicates.
 * Returns the values applied, in apply order.
 */
async function expectAppliedOnce(request: APIRequestContext, session: string, path: string): Promise<number[]> {
  const report = await audit(request);
  expect(report.summary.duplicates).toBe(0);
  const outcomes = (await commands(request)).filter((c) => c.owner?.handle === ownerHandle(session) && c.path === path && c.type === 'set');
  const applied = outcomes.filter((c) => c.result === 'applied').map((c) => c.commandId);
  expect(new Set(applied).size).toBe(applied.length);
  for (const c of outcomes) {
    expect(c.result === 'applied' || c.result === 'duplicate').toBe(true);
    if (c.result === 'duplicate') expect(applied).toContain(c.commandId);
  }
  const sets = report.entries.filter((e) => e.owner.handle === ownerHandle(session) && e.path === path && e.type === 'set');
  expect(sets.map((e) => e.commandId)).toEqual(applied);
  for (let i = 1; i < sets.length; i++) expect(sets[i]!.seq!).toBeGreaterThan(sets[i - 1]!.seq!);
  return sets.map((e) => e.value as number);
}

/**
 * Sends `presses` keyboard steps of -0.1 dB from 0 dB to a fader, runs
 * `fault` after the first third of them, and checks that every write was
 * applied once and the last written value is the committed one.
 */
async function writeAcross(
  page: Page,
  request: APIRequestContext,
  channel: string,
  fault: () => Promise<void>,
): Promise<void> {
  const path = `/channels/${channel}/fader`;
  const session = await debug(page, 'session');
  const slider = page.locator(`[role="slider"][data-path="${path}"]`);
  await slider.focus();
  await page.keyboard.press('Alt+0');
  await settled(page);
  await request.post('/__mock/audit/reset');
  const presses = 30;
  let faulted: Promise<void> = Promise.resolve();
  for (let i = 0; i < presses; i++) {
    if (i === presses / 3) faulted = fault();
    await page.keyboard.press('Shift+ArrowDown');
    await page.waitForTimeout(15);
  }
  await faulted;
  await expect(slider).toHaveAttribute('aria-valuenow', '-3');
  await settled(page);
  const values = await expectAppliedOnce(request, session, path);
  expect(values.length).toBeGreaterThan(0);
  for (let i = 1; i < values.length; i++) expect(values[i]!).toBeLessThan(values[i - 1]!);
  expect(values.at(-1)).toBe(-3);
  expect(await fader(request, channel)).toBe(-3);
  await expect(page.getByTestId(`fader-value-${channel}`)).toHaveText('-3.0');
}

test('two clients move different faders at the same time and see each other', { tag: '@mock' }, async ({ browser, baseURL, request }) => {
  await patch(request, '/channels/ch5/fader', -20);
  await patch(request, '/channels/ch6/fader', -20);
  const contexts = await Promise.all([0, 1].map(() => browser.newContext({ baseURL, viewport: { width: 1600, height: 1000 } })));
  try {
    const [a, b] = await Promise.all(contexts.map((c) => c.newPage()));
    await Promise.all([live(a!), live(b!)]);
    const sessions = await Promise.all([debug(a!, 'session'), debug(b!, 'session')]);
    expect(sessions[0]).not.toBe(sessions[1]);
    await Promise.all([dragFader(a!, '/channels/ch5/fader', -40), dragFader(b!, '/channels/ch6/fader', -70)]);
    await Promise.all([settled(a!), settled(b!)]);
    const ch5 = await fader(request, 'ch5');
    const ch6 = await fader(request, 'ch6');
    expect(ch5).toBeGreaterThan(-20);
    expect(ch6).toBeGreaterThan(ch5);
    for (const p of [a!, b!]) {
      await expect(p.getByTestId('fader-value-ch5')).toHaveText(formatDb(ch5));
      await expect(p.getByTestId('fader-value-ch6')).toHaveText(formatDb(ch6));
    }
    const entries = (await audit(request)).entries.filter((e) => e.type === 'set');
    expect(new Set(entries.filter((e) => e.path === '/channels/ch5/fader').map((e) => e.owner.handle))).toEqual(new Set([ownerHandle(sessions[0])]));
    expect(new Set(entries.filter((e) => e.path === '/channels/ch6/fader').map((e) => e.owner.handle))).toEqual(new Set([ownerHandle(sessions[1])]));
  } finally {
    await Promise.all(contexts.map((c) => c.close()));
  }
});

test('a fader held across a reload stays owned by the session and is released after resync', { tag: '@mock' }, async ({ page, baseURL, request }) => {
  await live(page);
  const path = '/channels/ch9/fader';
  const session = await debug(page, 'session');
  const { x, y } = await faderCenter(page, path);
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y - 30);
  await expect.poll(async () => (await serverState(request)).owners[path]?.handle).toBe(ownerHandle(session));
  await page.waitForFunction(() => window.__faderz?.outbox === 0);
  const value = await fader(request, 'ch9');
  await expect(page.getByTestId('fader-value-ch9')).toHaveText(formatDb(value));
  const other = await RawClient.open(baseURL!);
  try {
    // The reloaded document waits for its entry module until the other
    // client has been refused: the old page is gone, the new one has not
    // connected, and only the session lease holds the fader.
    let open!: () => void;
    const gate = new Promise<void>((r) => (open = r));
    let blocked!: () => void;
    const reached = new Promise<void>((r) => (blocked = r));
    await page.route(/\/src\/main\.ts(\?|$)/, async (route) => {
      blocked();
      await gate;
      await route.continue();
    });
    const reload = page.reload();
    await reached;
    await expect.poll(async () => (await stats(request)).filter((c) => c.handle === ownerHandle(session))).toEqual([]);
    const refused = await other.command({ type: 'set', path, value: value - 6 });
    expect(refused).toMatchObject({ type: 'error', code: 409, path, owner: { principal: 'anonymous', handle: ownerHandle(session), kind: 'ui' } });
    open();
    await reload;
    await page.unrouteAll();
    await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
    expect(await debug(page, 'session')).toBe(session);
    await expect.poll(async () => (await serverState(request)).owners[path]).toBeUndefined();
    const releases = (await audit(request)).entries.filter((e) => e.type === 'release' && e.path === path);
    expect(releases.map((e) => e.owner.handle)).toEqual([ownerHandle(session)]);
    expect(await fader(request, 'ch9')).toBe(value);
    await expect(page.getByTestId('fader-value-ch9')).toHaveText(formatDb(value));
    expect(await debug(page, 'touching')).toEqual([]);
  } finally {
    await page.mouse.up();
    other.close();
  }
});

test('fails over to the standby within 500 ms when the primary is killed, without losing or duplicating commands', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const from = (await debug(page, 'primaryReplica'))!;
  const standby = (await debug(page, 'standbyReplica'))!;
  const failovers = await debug(page, 'failovers');
  let killedAt = 0;
  await writeAcross(page, request, 'ch10', async () => {
    await watchSwitch(page, from);
    killedAt = Date.now();
    expect((await request.post(`/__mock/replicas/${from}/kill`)).ok()).toBe(true);
  });
  const ms = (await switchedAt(page)) - killedAt;
  test.info().annotations.push({ type: 'failover-ms', description: String(ms) });
  expect(ms).toBeLessThan(500);
  expect(await debug(page, 'primaryReplica')).toBe(standby);
  expect(await debug(page, 'failovers')).toBe(failovers + 1);
  expect(await debug(page, 'lastFailover')).toEqual({ from, to: standby, reason: 'lost' });
  await expect(page.locator(`[data-replica="${from}"]`)).toHaveAttribute('data-healthy', 'false');
  await standbyReady(page);
});

const faults = [
  { name: 'a hung replica that drops input', action: 'freeze', reason: 'dead', channel: 'ch12' },
  { name: 'a hung replica that still applies', action: 'freeze?mode=apply', reason: 'dead', channel: 'ch13' },
  { name: 'a partitioned replica', action: 'partition', reason: 'notCommitable', channel: 'ch14' },
] as const;

for (const f of faults) {
  test(`fails over from ${f.name} without losing or duplicating commands`, { tag: '@mock' }, async ({ page, request }) => {
    await live(page);
    const from = (await debug(page, 'primaryReplica'))!;
    const standby = (await debug(page, 'standbyReplica'))!;
    const failovers = await debug(page, 'failovers');
    let faultAt = 0;
    await writeAcross(page, request, f.channel, async () => {
      await watchSwitch(page, from);
      faultAt = Date.now();
      expect((await request.post(`/__mock/replicas/${from}/${f.action}`)).ok()).toBe(true);
    });
    const ms = (await switchedAt(page)) - faultAt;
    test.info().annotations.push({ type: 'failover-ms', description: String(ms) });
    // A dead primary is detected DEAD_MS after its last message.
    expect(ms).toBeLessThan(f.reason === 'dead' ? DEAD_MS + 500 : 500);
    expect(await debug(page, 'failovers')).toBe(failovers + 1);
    expect(await debug(page, 'lastFailover')).toEqual({ from, to: standby, reason: f.reason });
    await expect(page.getByTestId('failover')).toBeVisible();
    if (f.action === 'partition') {
      await expect(page.locator(`[data-replica="${from}"]`)).toHaveAttribute('data-commitable', 'false');
      await expect(page.getByTestId('quorum')).toHaveAttribute('data-commitable', 'true');
    }
  });
}

test('keeps a held fader owned across a replica kill', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const path = '/channels/ch20/fader';
  const session = await debug(page, 'session');
  const from = (await debug(page, 'primaryReplica'))!;
  const { x, y } = await faderCenter(page, path);
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y - 20);
  await expect.poll(async () => (await serverState(request)).owners[path]?.handle).toBe(ownerHandle(session));
  await request.post(`/__mock/replicas/${from}/kill`);
  await expect(page.getByTestId('failover')).toBeVisible();
  await page.mouse.move(x, y - 60);
  await expect(page.locator(`[data-replica="${from}"]`)).toHaveAttribute('data-healthy', 'false');
  // Longer than the 2 s session lease: the new primary connection renews it.
  await page.waitForTimeout(2500);
  expect((await serverState(request)).owners[path]).toMatchObject({ handle: ownerHandle(session), kind: 'ui' });
  await page.mouse.up();
  await settled(page);
  await expect.poll(async () => (await serverState(request)).owners[path]).toBeUndefined();
  await expect(page.getByTestId('fader-value-ch20')).toHaveText(formatDb(await fader(request, 'ch20')));
});

test('a hidden tab stops meter traffic after at most the credit cap and resumes when visible', { tag: '@meters' }, async ({ page, request }) => {
  await page.addInitScript(() => {
    let hidden = false;
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (hidden ? 'hidden' : 'visible') });
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
    (window as unknown as { __setHidden: (h: boolean) => void }).__setHidden = (h) => {
      hidden = h;
      document.dispatchEvent(new Event('visibilitychange'));
    };
  });
  await live(page);
  const session = await debug(page, 'session');
  const primary = async () => {
    const mine = (await stats(request)).filter((c) => c.handle === ownerHandle(session) && c.mode === 'primary');
    expect(mine).toHaveLength(1);
    return mine[0]!;
  };
  await expect.poll(async () => (await primary()).meterFramesSent).toBeGreaterThan(10);
  await page.evaluate(() => (window as unknown as { __setHidden: (h: boolean) => void }).__setHidden(true));
  await page.waitForFunction(() => window.__faderz?.ticking === false);
  const atHide = await primary();
  await expect.poll(async () => (await primary()).creditsOutstanding).toBe(0);
  const drained = await primary();
  expect(drained.meterFramesSent - atHide.meterFramesSent).toBeLessThanOrEqual(8);
  // Ten meter periods with no credit: nothing is sent, every period is skipped.
  await page.waitForTimeout(400);
  const later = await primary();
  expect(later.connection).toBe(atHide.connection);
  expect(later.meterFramesSent).toBe(drained.meterFramesSent);
  expect(later.metersSkippedNoCredit).toBeGreaterThanOrEqual(drained.metersSkippedNoCredit + 5);
  await page.evaluate(() => (window as unknown as { __setHidden: (h: boolean) => void }).__setHidden(false));
  await page.waitForFunction(() => window.__faderz?.ticking === true);
  await expect.poll(async () => (await primary()).meterFramesSent).toBeGreaterThan(later.meterFramesSent + 8);
  expect(await debug(page, 'connection')).toBe('Live');
});

test('mute toggles within 20 ms are each sent and applied in order', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const path = '/channels/ch1/mute';
  const before = (await serverState(request)).state.channels['ch1']!.mute;
  await page.evaluate((p) => {
    const b = document.querySelector<HTMLButtonElement>(`button[data-path="${p}"]`)!;
    b.click();
    b.click();
    b.click();
  }, path);
  await expect
    .poll(async () => (await audit(request)).entries.filter((e) => e.path === path).map((e) => e.value))
    .toEqual([!before, before, !before]);
  const applied = (await audit(request)).entries.filter((e) => e.path === path);
  for (let i = 1; i < applied.length; i++) {
    expect(applied[i]!.seq!).toBeGreaterThan(applied[i - 1]!.seq!);
    expect(applied[i]!.revision).toBeGreaterThanOrEqual(applied[i - 1]!.revision);
  }
  expect((await serverState(request)).state.channels['ch1']!.mute).toBe(!before);
});

test('a promote onto a lagging replica never shows an older value', { tag: '@mock' }, async ({ page, request, baseURL }) => {
  const ws = baseURL!.replace(/^http/, 'ws');
  await live(page, `/?endpoints=${ws}/__mock/replicas/r1/api/v1/ws,${ws}/__mock/replicas/r2/api/v1/ws`);
  expect(await debug(page, 'primaryReplica')).toBe('r1');
  expect(await debug(page, 'standbyReplica')).toBe('r2');
  expect((await request.post('/__mock/replicas/r2/lag?ms=1500')).ok()).toBe(true);
  const before = await fader(request, 'ch1');
  const target = before === -7 ? -8 : -7;
  const readout = page.getByTestId('fader-value-ch1');
  await page.evaluate(() => {
    const w = window as unknown as { __seen: string[] };
    w.__seen = [];
    const el = document.querySelector('[data-testid="fader-value-ch1"]')!;
    new MutationObserver(() => w.__seen.push(el.textContent ?? '')).observe(el, { subtree: true, characterData: true, childList: true });
  });
  await patch(request, '/channels/ch1/fader', target);
  await expect(readout).toHaveText(formatDb(target));
  expect((await request.post('/__mock/replicas/r1/kill')).ok()).toBe(true);
  await page.waitForFunction(() => window.__faderz?.primaryReplica === 'r2' && window.__faderz.connection === 'Live', null, { timeout: 10_000 });
  await expect(readout).toHaveText(formatDb(target));
  const seen = await page.evaluate(() => (window as unknown as { __seen: string[] }).__seen);
  expect(seen.slice(seen.indexOf(formatDb(target)))).not.toContain(formatDb(before));
});

test('without a replica that can commit the client stays put, keeps its controls and shows what it pressed as pending', { tag: '@mock' }, async ({ page, request }) => {
  test.setTimeout(60_000);
  const sent: Array<{ type: string; id?: string; path?: string }> = [];
  page.on('websocket', (ws) => ws.on('framesent', (f) => typeof f.payload === 'string' && sent.push(JSON.parse(f.payload))));
  await live(page);
  const mute = page.locator('button[data-path="/channels/ch3/mute"]');
  const before = await serverState(request);
  const was = before.state.channels['ch3']!.mute;
  const primary = (await debug(page, 'primaryReplica'))!;
  const standby = (await debug(page, 'standbyReplica'))!;
  const others = REPLICAS.filter((r) => r !== primary);
  for (const r of others) expect((await request.post(`/__mock/replicas/${r}/partition`)).ok()).toBe(true);

  const quorum = page.getByTestId('quorum');
  await expect(quorum).toHaveText('NO Q');
  await expect(quorum).toHaveAttribute('data-commitable', 'false');
  const banner = page.getByTestId('commit-banner');
  await expect(banner).toBeVisible();
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live');
  // The controls stay usable: the operator can still try.
  await expect(mute).toHaveAttribute('aria-disabled', 'false');
  await expect(page.locator('[role="slider"][data-path="/channels/ch3/fader"]')).toHaveAttribute('aria-disabled', 'false');
  await request.post('/__mock/audit/reset');
  await mute.click();
  await expect(mute).toHaveAttribute('aria-pressed', String(!was));
  await expect(mute).toHaveAttribute('data-pending', '');
  await expect(page.getByTestId('commit-pending')).toContainText('1');
  // Refused, not applied, and sent again until it expires; then the key
  // shows the committed state again.
  const sets = () => sent.filter((m) => m.type === 'set' && m.path === '/channels/ch3/mute');
  await expect.poll(() => sets().length).toBeGreaterThan(1);
  expect(new Set(sets().map((m) => m.id)).size).toBe(1);
  expect((await audit(request)).entries).toEqual([]);
  await expect(mute).toHaveAttribute('aria-pressed', String(was), { timeout: 5000 });
  await expect(mute).not.toHaveAttribute('data-pending');
  await expect(page.getByTestId('commit-pending')).toContainText('0');
  expect((await serverState(request)).state.channels['ch3']!.mute).toBe(was);
  // Every view's keys take the mark: TONE ON in Setup.
  await page.getByRole('button', { name: 'Setup', exact: true }).click();
  const tone = page.getByRole('button', { name: 'Line-up tone' });
  await expect(tone).toHaveAttribute('aria-disabled', 'false');
  await tone.click();
  await expect(tone).toHaveAttribute('aria-pressed', 'true');
  await expect(tone).toHaveAttribute('data-pending', '');
  await expect(tone).toHaveAttribute('aria-pressed', 'false', { timeout: 5000 });
  await expect(tone).not.toHaveAttribute('data-pending');
  expect((await serverState(request)).state.tone.on).toBe(false);
  await page.getByRole('button', { name: 'Mixer', exact: true }).click();
  // No replica could commit: no failover, and the standby was kept.
  expect(await debug(page, 'failovers')).toBe(0);
  expect(await debug(page, 'primaryReplica')).toBe(primary);
  expect(await debug(page, 'standbyReplica')).toBe(standby);

  for (const r of others) expect((await request.post(`/__mock/replicas/${r}/heal`)).ok()).toBe(true);
  await expect(quorum).toHaveText('Q');
  await expect(banner).toHaveCount(0);
  await mute.click();
  await settled(page);
  await expect.poll(async () => (await audit(request)).entries.map((e) => e.path)).toContain('/channels/ch3/mute');
  expect((await serverState(request)).state.channels['ch3']!.mute).toBe(!was);
  await patch(request, '/channels/ch3/mute', was);
});

test('with every replica down the client disables every write control and sends nothing written then', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  const faderPath = '/channels/ch4/fader';
  const slider = page.locator(`[role="slider"][data-path="${faderPath}"]`);
  const mute = page.locator('button[data-path="/channels/ch4/mute"]');
  await expect(slider).toHaveAttribute('aria-disabled', 'false');
  const before = await serverState(request);
  for (const r of REPLICAS) expect((await request.post(`/__mock/replicas/${r}/kill`)).ok()).toBe(true);
  await expect.poll(() => debug(page, 'connection')).toMatch(/^(Connecting|Offline)$/);
  // The quorum slot says why writes are off: the link, not the quorum.
  const lock = page.getByTestId('quorum');
  await expect(lock).toHaveText('LOCK');
  await expect(lock).toHaveAttribute('title', /^Every write control is disabled: (connecting|no replica reachable)$/);
  await expect(slider).toHaveAttribute('aria-disabled', 'true');
  await expect(mute).toHaveAttribute('aria-disabled', 'true');
  await expect(page.locator('[role="slider"][aria-disabled="false"]')).toHaveCount(0);

  // force: Playwright waits for an aria-disabled control to become enabled.
  await mute.click({ force: true });
  await slider.focus();
  await page.keyboard.press('PageUp');
  expect(await debug(page, 'outbox')).toBe(0);
  await expect(mute).toHaveAttribute('aria-pressed', String(before.state.channels['ch4']!.mute));

  await request.post('/__mock/audit/reset');
  for (const r of REPLICAS) expect((await request.post(`/__mock/replicas/${r}/restore`)).ok()).toBe(true);
  await expect.poll(() => debug(page, 'connection'), { timeout: 10_000 }).toBe('Live');
  await expect(lock).toHaveText('Q');
  await expect(slider).toHaveAttribute('aria-disabled', 'false');
  await settled(page);
  expect((await audit(request)).entries).toEqual([]);
  expect((await serverState(request)).state.channels['ch4']).toEqual(before.state.channels['ch4']);
});

test('sends the same ?session= on the primary, the standby and the reconnect after a failover', { tag: '@mock' }, async ({ page, request }) => {
  const urls: string[] = [];
  page.on('websocket', (ws) => {
    if (ws.url().includes('/api/v1/ws')) urls.push(ws.url());
  });
  await live(page);
  await expect.poll(() => urls.length).toBeGreaterThanOrEqual(2);
  const primary = await debug(page, 'primaryReplica');
  await request.post(`/__mock/replicas/${primary}/kill`);
  await expect.poll(() => debug(page, 'failovers')).toBeGreaterThan(0);
  await expect.poll(() => urls.length).toBeGreaterThanOrEqual(3);
  const sessions = new Set(urls.map((u) => new URL(u).searchParams.get('session')));
  expect(sessions.size).toBe(1);
  expect([...sessions][0]).toMatch(/^[A-Za-z0-9_-]{22,128}$/);
  expect([...sessions][0]).toBe(await debug(page, 'session'));
});

test('a client of a rewound cluster shows it is ahead at once and takes the cluster state on request', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch19/fader', -11);
  const written = (await serverState(request)).revision;
  await expect.poll(() => debug(page, 'revision')).toBeGreaterThanOrEqual(written);
  const mine = (await debug(page, 'revision')) as number;
  // A restore from an older backup: a new epoch, and every socket closed.
  await request.post('/__mock/rewind?by=2');
  const banner = page.getByTestId('ahead');
  await expect(banner).toBeVisible();
  await expect(banner).toContainText(`revision ${mine} of an earlier history`);
  expect(await debug(page, 'connection')).toBe('Ahead');
  expect(await debug(page, 'revision')).toBe(mine);
  await expect(page.getByTestId('quorum')).toHaveAttribute('title', 'Every write control is disabled: this client is ahead of the cluster');
  await banner.getByRole('button', { name: 'TAKE CLUSTER STATE' }).click();
  await expect(banner).toBeHidden();
  await expect.poll(() => debug(page, 'connection')).toBe('Live');
  await expect.poll(async () => (await debug(page, 'revision')) === (await serverState(request)).revision).toBe(true);
});

test('a client never resumes onto a restored cluster that has committed past its revision', { tag: '@mock' }, async ({ page, request }) => {
  await live(page);
  await patch(request, '/channels/ch19/fader', -11);
  const written = (await serverState(request)).revision;
  await expect.poll(() => debug(page, 'revision')).toBeGreaterThanOrEqual(written);
  await expect(page.getByTestId('fader-value-ch19')).toHaveText(formatDb(-11));
  const mine = (await debug(page, 'revision')) as number;
  for (const id of REPLICAS) expect((await request.post(`/__mock/replicas/${id}/kill`)).ok()).toBe(true);
  await request.post('/__mock/rewind?by=2');
  // Every replica is down, so the API refuses writes (503): the restored
  // cluster's newer history is written as a restore would write it.
  for (const v of [-30, -31, -32]) {
    const res = await request.post('/__mock/write', { data: { changes: [{ path: '/channels/ch19/fader', value: v }] } });
    expect(res.ok()).toBe(true);
  }
  const restored = (await serverState(request)).revision;
  expect(restored).toBeGreaterThan(mine);
  for (const id of REPLICAS) expect((await request.post(`/__mock/replicas/${id}/restore`)).ok()).toBe(true);
  const banner = page.getByTestId('ahead');
  await expect(banner).toBeVisible();
  await expect(banner).toContainText(`revision ${mine} of an earlier history`);
  expect(await debug(page, 'connection')).toBe('Ahead');
  expect(await debug(page, 'revision')).toBe(mine);
  await expect(page.getByTestId('fader-value-ch19')).toHaveText(formatDb(-11));
  await banner.getByRole('button', { name: 'TAKE CLUSTER STATE' }).click();
  await expect.poll(() => debug(page, 'connection')).toBe('Live');
  await expect(page.getByTestId('fader-value-ch19')).toHaveText(formatDb(-32));
  await expect.poll(async () => (await debug(page, 'revision')) === (await serverState(request)).revision).toBe(true);
});

test('connects on its first socket over a link whose requests and upgrade take 4 s, and never shows Offline', { tag: '@mock' }, async ({ page, request }) => {
  // The page's session request is as slow as the upgrade, so the deadline
  // it measures for the first socket leaves room for a 4 s open.
  expect((await request.post('/__mock/slow-link?ms=4000')).ok()).toBe(true);
  const sockets: string[] = [];
  page.on('websocket', (ws) => {
    if (ws.url().includes('/api/v1/ws')) sockets.push(ws.url());
  });
  await page.addInitScript(() => {
    const seen: string[] = ((window as unknown as { __links: string[] }).__links = []);
    const t = setInterval(() => {
      const s = window.__faderz?.connection;
      if (s !== undefined && seen.at(-1) !== s) seen.push(s);
      if (s === 'Live') clearInterval(t);
    }, 20);
  });
  await page.goto('/');
  await expect(page.getByTestId('link')).toHaveAttribute('data-state', 'Live', { timeout: 20_000 });
  const links = await page.evaluate(() => (window as unknown as { __links: string[] }).__links);
  expect(links).not.toContain('Offline');
  // The primary was the first socket; the standby the second, at most.
  expect(sockets.length).toBeLessThanOrEqual(2);
});
