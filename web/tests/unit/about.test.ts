import { describe, expect, it } from 'vitest';
import { latest, type Load } from '../../src/lib/about.svelte';

/** A promise and the keys that settle it. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: Error) => void;
  const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
}

describe('the About dialog load', () => {
  it('shows the latest load alone when an older one ends after it', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first, second];
    const shown: Load<string>[] = [];
    const loader = latest(() => answers.shift()!.promise, (l) => shown.push(l));
    const one = loader.run();
    const two = loader.run();
    second.resolve('new');
    await two;
    first.resolve('old');
    await one;
    expect(shown).toEqual([{ kind: 'busy' }, { kind: 'busy' }, { kind: 'done', value: 'new' }]);
  });

  it('shows no failure of an older load over a newer one', async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const answers = [first, second];
    const shown: Load<string>[] = [];
    const loader = latest(() => answers.shift()!.promise, (l) => shown.push(l));
    const one = loader.run();
    const two = loader.run();
    second.resolve('new');
    first.reject(new Error('late failure'));
    await Promise.all([one, two]);
    expect(shown.at(-1)).toEqual({ kind: 'done', value: 'new' });
    expect(shown.some((l) => l.kind === 'error')).toBe(false);
  });

  it('sets nothing for a load under way once cancelled, as when the dialog closes', async () => {
    const answer = deferred<string>();
    const shown: Load<string>[] = [];
    const loader = latest(() => answer.promise, (l) => shown.push(l));
    const run = loader.run();
    loader.cancel();
    answer.resolve('late');
    await run;
    expect(shown).toEqual([{ kind: 'busy' }]);
  });

  it('shows a failure of the latest load', async () => {
    const shown: Load<string>[] = [];
    await latest(() => Promise.reject(new Error('the replica broke')), (l) => shown.push(l)).run();
    expect(shown).toEqual([{ kind: 'busy' }, { kind: 'error', message: 'the replica broke' }]);
  });
});
