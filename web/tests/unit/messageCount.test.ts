import { describe, expect, it } from 'vitest';
import { INITIAL_STATUS, MixerStore } from '../../src/lib/state/mixer.svelte';
import { SyncEngine } from '../../src/lib/sync/engine';
import type { Patch, ToWorker } from '../../src/lib/sync/messages';
import { exampleShape, initialState } from '../../dev/mock/shape';
import { FakeClock, FakeSocket } from './fakes';

const LIVE = { ...INITIAL_STATUS, link: 'Live' as const };

/** A store fed by an engine as the worker runs it: the worker starts the engine on the init message and hands it every later one. */
function wired() {
  const patches: Patch[] = [];
  const engine = new SyncEngine({
    endpoints: ['ws://svc/api/v1/ws'],
    session: 's',
    seqBase: 1,
    socket: (url) => new FakeSocket(url),
    clock: new FakeClock(),
    post: (p) => patches.push(structuredClone(p)),
    random: () => 0,
  });
  const s = new MixerStore();
  s.attach({ post: (m: ToWorker) => engine.input(m) });
  const shape = exampleShape();
  s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
  return { s, engine, patches };
}

describe('message numbers', () => {
  // settled() in the e2e support waits for processed >= commanded. The
  // engine must number a command as the page does, so that a patch built
  // before the command never satisfies the wait.
  it('numbers a command as the engine does, so only a patch after it covers it', () => {
    const { s, engine, patches } = wired();
    s.tick();
    s.tick();
    s.set('/channels/ch1/mute', true);
    const { commanded } = s.messages;
    const before = { type: 'patch' as const, input: commanded - 1, changes: [], owners: [] };
    s.apply(before);
    expect(s.messages.processed).toBeLessThan(commanded);
    engine.input({ type: 'tick' });
    const after = patches.at(-1)!;
    // The tick after the set: the engine had processed the set by then.
    expect(after.input).toBe(commanded + 1);
    s.apply(after);
    expect(s.messages.processed).toBeGreaterThanOrEqual(commanded);
  });

  it('counts no message sent before the worker is attached', () => {
    const s = new MixerStore();
    s.tick();
    s.attach({ post: () => {} });
    const shape = exampleShape();
    s.apply({ type: 'patch', input: 0, snapshot: { shape, state: initialState(shape), owners: {} }, changes: [], owners: [], status: LIVE });
    s.tick();
    s.set('/channels/ch1/mute', true);
    expect(s.messages.commanded).toBe(2);
  });
});
