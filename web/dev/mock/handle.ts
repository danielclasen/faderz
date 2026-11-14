// Owner handle of a session id (model.schema.json#/$defs/OwnerHandle),
// computed with node:crypto, independently of the client's own SHA-256.

import { createHash } from 'node:crypto';
import type { OwnerHandle, SessionId } from '../../src/lib/contract/types.ts';

export function handleOf(session: SessionId): OwnerHandle {
  return createHash('sha256').update(session, 'ascii').digest().subarray(0, 16).toString('base64url');
}
