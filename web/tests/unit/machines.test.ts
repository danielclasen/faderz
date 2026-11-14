import { describe, expect, it } from 'vitest';
import {
  COMMAND,
  COMMAND_EVENTS,
  COMMAND_STATES,
  LINK,
  LINK_EVENTS,
  LINK_STATES,
  STANDBY,
  STANDBY_EVENTS,
  STANDBY_STATES,
  TOUCH,
  TOUCH_EVENTS,
  TOUCH_STATES,
  step,
  type CommandState,
  type Table,
} from '../../src/lib/sync/machines';

// Each expected table is written out in full: every state times every
// event, with the next state. A row lists the events in the order of the
// header comment above it.

function check<S extends string, E extends string>(
  table: Table<S, E>,
  states: readonly S[],
  events: readonly E[],
  expected: Record<S, readonly S[]>,
): void {
  expect(Object.keys(expected).sort()).toEqual([...states].sort());
  expect(Object.keys(table).sort()).toEqual([...states].sort());
  for (const s of states) {
    const row = expected[s];
    expect(row.length, `row ${s}`).toBe(events.length);
    events.forEach((e, i) => expect(step(table, s, e), `${s} --${e}-->`).toBe(row[i]));
  }
}

describe('connection machine', () => {
  it('has the expected event and state lists', () => {
    expect(LINK_EVENTS).toEqual([
      'open', 'synced', 'resync', 'beatLate', 'beat', 'beatsMissed', 'lost', 'notCommitable', 'promoted', 'noStandby', 'exhausted', 'retry', 'ahead', 'adopt',
    ]);
    expect(LINK_STATES).toEqual(['Connecting', 'Syncing', 'Live', 'Suspect', 'FailingOver', 'Resyncing', 'Ahead', 'Offline']);
  });

  it('follows the full transition table', () => {
    // open, synced, resync, beatLate, beat, beatsMissed, lost, notCommitable, promoted, noStandby, exhausted, retry, ahead, adopt
    check(LINK, LINK_STATES, LINK_EVENTS, {
      Connecting: ['Syncing', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Connecting', 'Offline', 'Connecting', 'Connecting', 'Connecting'],
      Syncing: ['Syncing', 'Live', 'Syncing', 'Syncing', 'Syncing', 'Connecting', 'Connecting', 'Syncing', 'Syncing', 'Syncing', 'Syncing', 'Syncing', 'Ahead', 'Syncing'],
      Live: ['Live', 'Live', 'Resyncing', 'Suspect', 'Live', 'FailingOver', 'FailingOver', 'FailingOver', 'Live', 'Live', 'Live', 'Live', 'Live', 'Live'],
      Suspect: ['Suspect', 'Suspect', 'Resyncing', 'Suspect', 'Live', 'FailingOver', 'FailingOver', 'FailingOver', 'Suspect', 'Suspect', 'Suspect', 'Suspect', 'Suspect', 'Suspect'],
      FailingOver: ['FailingOver', 'FailingOver', 'FailingOver', 'FailingOver', 'FailingOver', 'FailingOver', 'FailingOver', 'FailingOver', 'Resyncing', 'Connecting', 'FailingOver', 'FailingOver', 'FailingOver', 'FailingOver'],
      Resyncing: ['Resyncing', 'Live', 'Resyncing', 'Resyncing', 'Resyncing', 'FailingOver', 'FailingOver', 'FailingOver', 'Resyncing', 'Resyncing', 'Resyncing', 'Resyncing', 'Ahead', 'Resyncing'],
      Ahead: ['Ahead', 'Ahead', 'Ahead', 'Ahead', 'Ahead', 'Connecting', 'Connecting', 'Ahead', 'Ahead', 'Ahead', 'Ahead', 'Ahead', 'Ahead', 'Resyncing'],
      Offline: ['Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Offline', 'Connecting', 'Offline', 'Offline'],
    });
  });
});

describe('standby machine', () => {
  it('has the expected event and state lists', () => {
    expect(STANDBY_EVENTS).toEqual(['start', 'open', 'synced', 'unsuitable', 'lost', 'beatsMissed', 'promote']);
    expect(STANDBY_STATES).toEqual(['Idle', 'Connecting', 'Syncing', 'Ready']);
  });

  it('follows the full transition table', () => {
    // start, open, synced, unsuitable, lost, beatsMissed, promote
    check(STANDBY, STANDBY_STATES, STANDBY_EVENTS, {
      Idle: ['Connecting', 'Idle', 'Idle', 'Idle', 'Idle', 'Idle', 'Idle'],
      Connecting: ['Connecting', 'Syncing', 'Connecting', 'Connecting', 'Idle', 'Idle', 'Connecting'],
      Syncing: ['Syncing', 'Syncing', 'Ready', 'Idle', 'Idle', 'Idle', 'Syncing'],
      Ready: ['Ready', 'Ready', 'Ready', 'Idle', 'Idle', 'Idle', 'Idle'],
    });
  });
});

describe('command machine', () => {
  it('has the expected event and state lists', () => {
    expect(COMMAND_EVENTS).toEqual(['transmit', 'ack', 'reject', 'stale', 'supersede', 'failover', 'timeout', 'notApplied', 'expire']);
    expect(COMMAND_STATES).toEqual(['Queued', 'Sent', 'Acked', 'Rejected', 'Stale', 'Superseded', 'Expired']);
  });

  it('follows the full transition table', () => {
    // transmit, ack, reject, stale, supersede, failover, timeout, notApplied, expire
    const end = (s: CommandState) => Array<CommandState>(9).fill(s);
    check(COMMAND, COMMAND_STATES, COMMAND_EVENTS, {
      Queued: ['Sent', 'Acked', 'Rejected', 'Stale', 'Superseded', 'Queued', 'Queued', 'Queued', 'Expired'],
      Sent: ['Sent', 'Acked', 'Rejected', 'Stale', 'Sent', 'Queued', 'Queued', 'Queued', 'Expired'],
      Acked: end('Acked'),
      Rejected: end('Rejected'),
      Stale: end('Stale'),
      Superseded: end('Superseded'),
      Expired: end('Expired'),
    });
  });
});

describe('touch machine', () => {
  it('has the expected event and state lists', () => {
    expect(TOUCH_EVENTS).toEqual(['down', 'up', 'released', 'refused', 'failover']);
    expect(TOUCH_STATES).toEqual(['Idle', 'Touching', 'Releasing']);
  });

  it('follows the full transition table; a failover changes no touch', () => {
    // down, up, released, refused, failover
    check(TOUCH, TOUCH_STATES, TOUCH_EVENTS, {
      Idle: ['Touching', 'Idle', 'Idle', 'Idle', 'Idle'],
      Touching: ['Touching', 'Releasing', 'Touching', 'Idle', 'Touching'],
      Releasing: ['Touching', 'Releasing', 'Idle', 'Releasing', 'Releasing'],
    });
  });
});
