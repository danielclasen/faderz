// Whether a replica serves a newer faderz build than the one this page was
// loaded from: a rolled replica runs a newer UI and API, and the page
// should be reloaded to take them. An older or equal one, as a page from a
// new replica reconnecting to one not rolled yet sees, or a rollback, is
// not news.

import type { Build } from './contract/types';

declare const __FADERZ_VERSION__: string;

/** The faderz version this bundle was built with. */
export const OWN_VERSION: string = __FADERZ_VERSION__;

/** Semantic version precedence: below 0, 0 or above 0; null when either does not parse. */
export function compareVersions(a: string, b: string): number | null {
  const parse = (v: string) => {
    const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(v);
    return m ? { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4]?.split('.') ?? [] } : null;
  };
  const x = parse(a);
  const y = parse(b);
  if (!x || !y) return null;
  for (let i = 0; i < 3; i++) if (x.core[i] !== y.core[i]) return x.core[i]! - y.core[i]!;
  // A release is above its prereleases.
  if (x.pre.length === 0 || y.pre.length === 0) return y.pre.length - x.pre.length;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined || q === undefined) return p === undefined ? -1 : 1;
    if (p === q) continue;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) return Number(p) - Number(q);
    if (pn !== qn) return pn ? -1 : 1;
    return p < q ? -1 : 1;
  }
  return 0;
}

/**
 * The faderz version a replica reports when it is newer than this page's,
 * and not the one the operator last dismissed; null otherwise.
 */
export function newVersion(served: Build | null | undefined, dismissed: string | null, own = OWN_VERSION): string | null {
  const v = served?.faderz;
  if (!v || v === dismissed) return null;
  return (compareVersions(v, own) ?? 0) > 0 ? v : null;
}
