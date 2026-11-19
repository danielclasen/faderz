// Whether the About / What's new dialog is open: opened from the version
// lines of the connection details and the account menu, which close
// around it, so the dialog lives at the app's root (App.svelte).

export const about = $state<{ open: boolean }>({ open: false });

export function openAbout(): void {
  about.open = true;
}

/** What a load of the dialog's content is: under way, failed, or done. */
export type Load<T> = { kind: 'busy' } | { kind: 'error'; message: string } | { kind: 'done'; value: T };

/**
 * Loads with `fetch` and hands each state to `set`, the latest load's
 * alone: a load started before another, as by closing and opening the
 * dialog again or pressing Try again twice, sets nothing once it ends,
 * however late. `cancel` makes every load under way stale.
 */
export function latest<T>(fetch: () => Promise<T>, set: (load: Load<T>) => void): { run: () => Promise<void>; cancel: () => void } {
  let current = 0;
  return {
    async run() {
      const mine = ++current;
      set({ kind: 'busy' });
      let next: Load<T>;
      try {
        next = { kind: 'done', value: await fetch() };
      } catch (e) {
        next = { kind: 'error', message: e instanceof Error ? e.message : String(e) };
      }
      if (mine === current) set(next);
    },
    cancel() {
      current++;
    },
  };
}
