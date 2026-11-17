// The rows of the users panel: the sessions connected to the primary's
// replica, and the holders of controls connected elsewhere.

import type { Owner, Owners, OwnerHandle, PresentSession, Selection } from './contract/types';

export interface UserRow {
  handle: OwnerHandle;
  /**
   * Display name of who it is signed in as. A holder connected to another
   * replica shows OTHER_REPLICA: its owner record carries only the
   * principal, which in oidc mode can be an email address or subject id.
   */
  user: string;
  kind: string;
  /** None for a holder connected elsewhere: its owner record does not carry it. */
  role: PresentSession['role'] | null;
  you: boolean;
  /** Control paths it holds. */
  holds: string[];
  /** The element it has selected, if any. */
  selected: string | null;
  /** Not connected to the primary's replica, known only from what it holds. */
  elsewhere: boolean;
}

/**
 * One row per session: every present one but the consoles (`consoles`,
 * their handles), this screen (`you`) first, then the holders of a control
 * that are not present, which are connected to another replica. A session
 * is matched to what it holds and selects by its handle: presence carries
 * no principal.
 */
/** Shown for a holder connected to another replica, whose name this replica does not have. */
export const OTHER_REPLICA = 'on another replica';

export function userRows(
  presence: PresentSession[],
  owners: Owners,
  selections: Selection[],
  consoles: OwnerHandle[],
  you: OwnerHandle | null,
): UserRow[] {
  const holders = new Map<OwnerHandle, { owner: Owner; paths: string[] }>();
  for (const [path, owner] of Object.entries(owners)) {
    if (!owner) continue;
    const e = holders.get(owner.handle) ?? { owner, paths: [] };
    e.paths.push(path);
    holders.set(owner.handle, e);
  }
  const selected = (h: OwnerHandle) => selections.find((s) => s.session.handle === h)?.element ?? null;
  const rows: UserRow[] = presence
    .filter((p) => !(p.kind === 'console' && consoles.includes(p.handle)))
    .map((p) => ({
      handle: p.handle,
      user: p.user,
      kind: p.kind,
      role: p.role,
      you: p.handle === you,
      holds: holders.get(p.handle)?.paths ?? [],
      selected: selected(p.handle),
      elsewhere: false,
    }));
  const present = new Set(presence.map((p) => p.handle));
  for (const [handle, { owner, paths }] of holders) {
    if (present.has(handle) || (owner.kind === 'console' && consoles.includes(handle))) continue;
    rows.push({ handle, user: OTHER_REPLICA, kind: owner.kind, role: null, you: handle === you, holds: paths, selected: selected(handle), elsewhere: true });
  }
  return rows.sort((a, b) => Number(b.you) - Number(a.you));
}
