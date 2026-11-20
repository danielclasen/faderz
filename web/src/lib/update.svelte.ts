// The newer faderz build a replica serves, shared by the header's UPDATE
// key and, on the narrowest screens, the account menu that stands in for
// it: LATER in either puts that version off in both.

import { newVersion } from './newVersion';
import type { Build } from './contract/types';

class Update {
  /** The newest version the operator put off with LATER. */
  later = $state<string | null>(null);

  /** A newer version than this page's that `build` names, or null. */
  version(build: Build | null | undefined): string | null {
    return newVersion(build, null);
  }

  /** Whether that version is news: newer, and not put off. */
  fresh(build: Build | null | undefined): boolean {
    const v = this.version(build);
    return v !== null && v !== this.later;
  }
}

export const update = new Update();
