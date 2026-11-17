// Numeric constants of the WebSocket contract in api/ws/*.schema.json,
// shared by the client and the dev mock.

/** Both sides send ping on every connection at this period. */
export const HEARTBEAT_MS = 250;
/** A peer silent for this long (1.5 heartbeats) is suspect; nothing is switched. */
export const SUSPECT_MS = 375;
/** A peer silent for this long (3 heartbeats) is dead; its connection is closed. */
export const DEAD_MS = 750;
/** A session lease expires this long after the last message of the session. */
export const LEASE_MS = 2000;

/** Meter period of a replica: 25 frames per second. */
export const METER_PERIOD_MS = 40;
/** Outstanding meter credits per connection; grants beyond it are discarded. */
export const METER_CREDIT_CAP = 8;

/** A client sends at most this many writes per second to one control. */
export const MAX_WRITES_PER_SECOND = 50;
/** Commands a client keeps sent and unanswered per connection; the server reads no further. */
export const MAX_COMMANDS_IN_FLIGHT = 64;
/**
 * A client sends no command, first or again, later than this after
 * issuing it (its failover bound), well within the 10 s the server
 * remembers command ids and seqs.
 */
export const COMMAND_EXPIRY_MS = 3000;

/**
 * The cluster forwards at most one control request per asking session and
 * control in this time; it drops the others. A client sends no more often.
 */
export const REQUEST_INTERVAL_MS = 3000;
/** The cluster sends at most one selections and one presence message per connection in this time. */
export const SELECTIONS_INTERVAL_MS = 100;

/** First byte of a binary meter frame, ASCII 'M'. */
export const METER_FRAME_MAGIC = 0x4d;
/** Format version in the second byte of a binary meter frame. */
export const METER_FRAME_VERSION = 3;
/** Int16 meter value meaning null, a loudness or correlation of -Infinity, or no measurement. */
export const METER_NULL = -32768;
/** Int16 meter value of a leg of digital silence, -Infinity: -120 dB, the floor of a leg. */
export const METER_SILENCE = -12000;
/** Largest and smallest Int16 meter values other than METER_NULL. */
export const METER_MAX = 32767;
export const METER_MIN = -32767;
