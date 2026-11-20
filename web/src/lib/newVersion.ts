// Whether a replica serves another faderz build than the one this page was
// loaded from: a rolled replica runs a newer UI and API, and the page
// should be reloaded to take them.

import type { Build } from './contract/types';

declare const __FADERZ_VERSION__: string;

/** The faderz version this bundle was built with. */
export const OWN_VERSION: string = __FADERZ_VERSION__;

/**
 * The faderz version a replica reports when it is not this page's, and
 * not the one the operator last dismissed; null otherwise.
 */
export function newVersion(served: Build | null | undefined, dismissed: string | null, own = OWN_VERSION): string | null {
  const v = served?.faderz;
  return v && v !== own && v !== dismissed ? v : null;
}
