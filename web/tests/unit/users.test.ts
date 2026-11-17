import { describe, expect, it } from 'vitest';
import { userRows } from '../../src/lib/presence';
import type { Owner, PresentSession } from '../../src/lib/contract/types';
import { ownerHandle } from '../../src/lib/net/handle';

const AT = '2026-11-17T09:00:00.000Z';
const ME = ownerHandle('bWUtc2Vzc2lvbi0xMjM0NTY3');
const ALICE = ownerHandle('YWxpY2Utc2Vzc2lvbi0xMjM');
const FAR = ownerHandle('ZmFyLXNlc3Npb24tMTIzNDU');
const DESK = ownerHandle('ZGVzay1zZXNzaW9uLTEyMzQ');
const present = (handle: string, user: string, kind: 'ui' | 'console' = 'ui', role: 'operator' | 'viewer' = 'operator'): PresentSession => ({
  handle,
  kind,
  user,
  role,
  since: AT,
  active: AT,
});
const owner = (handle: string, principal = 'anonymous', kind: Owner['kind'] = 'ui'): Owner => ({ principal, handle, kind, connection: 'c1' });

describe('users panel rows', () => {
  it('lists every present session, this screen first, with what it holds and has selected', () => {
    const rows = userRows(
      [present(ALICE, 'alice', 'ui', 'viewer'), present(ME, 'daniel')],
      { '/channels/ch1/fader': owner(ALICE, 'alice'), '/channels/ch2/mute': owner(ALICE, 'alice') },
      [{ session: owner(ALICE, 'alice'), element: 'ch3' }],
      [],
      ME,
    );
    expect(rows.map((r) => [r.user, r.you, r.role])).toEqual([
      ['daniel', true, 'operator'],
      ['alice', false, 'viewer'],
    ]);
    expect(rows[1]!.holds).toEqual(['/channels/ch1/fader', '/channels/ch2/mute']);
    expect(rows[1]!.selected).toBe('ch3');
    expect(rows[0]!.holds).toEqual([]);
    expect(rows[0]!.selected).toBeNull();
  });

  it('lists a session that holds nothing: presence, not ownership, makes a row', () => {
    const rows = userRows([present(ME, 'daniel'), present(ALICE, 'alice')], {}, [], [], ME);
    expect(rows).toHaveLength(2);
  });

  it('leaves the consoles to their own rows, and lists a holder on another replica once', () => {
    const rows = userRows(
      [present(ME, 'daniel'), present(DESK, 'bridge', 'console')],
      { '/channels/ch4/fader': owner(FAR), '/channels/ch5/fader': owner(FAR), '/channels/ch6/fader': owner(DESK, 'bridge', 'console') },
      [],
      [DESK],
      ME,
    );
    expect(rows.map((r) => [r.handle, r.elsewhere])).toEqual([
      [ME, false],
      [FAR, true],
    ]);
    expect(rows[1]!.holds).toEqual(['/channels/ch4/fader', '/channels/ch5/fader']);
    expect(rows[1]!.role).toBeNull();
  });
});
