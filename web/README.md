# faderz web UI

Svelte 5 + Vite + TypeScript client of the faderz API. The contract it
speaks is in `../api`: `openapi.yaml` for REST and `ws/*.schema.json`
for the WebSocket at `/api/v1/ws`.

## Requirements

Node 22 and npm.

```sh
npm ci
npx playwright install chromium   # once, for the e2e tests
```

## Development

```sh
npm run dev
```

serves the UI with a mock faderz server on <http://localhost:5173>.
The mock (`dev/mock/`) is dev-only: Vite loads it only under
`npm run dev` and `vite preview`, and `npm run build` contains none of it. It simulates
three replicas sharing one state, binary meter frames against credit,
loudness, AFV tally cuts every 8 s and console activity, and validates
every message it sends and receives against the contract; each meter
frame is decoded and its values validated before it is sent.

- `FADERZ_MOCK_AUTOMATION=0` turns off the tally cuts and console
  activity, so nothing changes state but clients and REST calls.
- `FADERZ_MOCK_CHANNELS=<n>` sets the input channel count (default 29):
  mono channels `ch1`... followed by up to four stereo channels and one
  5.1 channel.

The striped MOCK button in the status bar kills, hangs, partitions and
restores replicas. The same controls answer JSON under `/__mock`:

| Method and path | Effect |
| --- | --- |
| `GET /__mock/replicas` | `[{id, role, healthy, frozen, partitioned, clients}]` |
| `POST /__mock/replicas/<id>/kill` | closes its sockets; new ones get 503 until restart |
| `POST /__mock/replicas/<id>/restart` | serves again: restarts a killed replica, closes the sockets of a hung one, heals a partition; `restore` is the same |
| `POST /__mock/replicas/<id>/freeze` | stays connected, sends and applies nothing; `?mode=apply` still applies |
| `POST /__mock/replicas/<id>/unfreeze` | resumes on the open sockets |
| `POST /__mock/replicas/<id>/partition` | reports `commitable: false`, answers every command with error 503 (not applied) and every hello or resync at once with its state marked `stale`, renews no lease |
| `POST /__mock/replicas/<id>/heal` | ends the partition |
| `POST /__mock/replicas/<id>/lag?ms=<n>` | log entries reach the replica `n` ms late; it answers a hello or resync carrying `lastRevision`, and runs commands after a promote, only once it has applied that revision; a hello or resync it has held for 2 s it answers with its state marked `stale`; `ms=0` catches up |
| `POST /__mock/blocks/hold` | keeps the current processing block open, so every WebSocket command joins one log entry until a REST request or a hello or resync ends it |
| `POST /__mock/blocks/flush` | ends the hold and commits the open block |
| `GET /__mock/stats` | per live connection: `connection, handle, kind, mode, replica, meterFramesSent, metersSkippedNoCredit, creditsOutstanding, messagesIn, messagesOut, openedAt`; `totals` over all connections since start or reset |
| `POST /__mock/stats/reset` | zeroes the counters |
| `GET /__mock/audit` | applied WebSocket commands in apply order, `{owner, commandId, type, seq, path, value, revision, replica}`, and `summary: {total, duplicates}`, where `duplicates` counts command ids applied more than once |
| `GET /__mock/commands` | every command outcome, REST included |
| `POST /__mock/audit/reset` | clears both command logs |
| `POST /__mock/drop-clients` | closes every socket |
| `POST /__mock/skip-revision` | advances the revision without a delta |
| `POST /__mock/rewind?by=<n>` | rolls the cluster back `n` revisions, as a restore from an older backup would: a new epoch and every socket closed, so a client that held state reconnects with the old epoch and gets a snapshot of the new one |

`/__mock/replicas/<id>/api/v1/ws` connects to one replica, and
`/__mock/replicas/<id>/api/v1/...` serves the REST API through it, 503
while it is killed or hung; `/api/v1/ws` picks a serving replica at
random. A `/__mock` request whose `Host` is not an IP address,
`localhost` or the `--host` name, and a `/__mock` POST that carries an
`Origin` other than the dev server's own, are refused with 403. The mock remembers a command id for
10 s after it applied and the last seq of a session and control for
13 s after the session's lease expired; its command logs keep the
latest 10000 entries.

HTTPS for the dev server: set `FADERZ_DEV_TLS_CERT` and `FADERZ_DEV_TLS_KEY`
to PEM files, for example a self-signed pair; the UI then uses `wss://`.
Plain HTTP works too, also on LAN addresses that are not secure
contexts.

The UI connects to `/api/v1/ws` on its own origin. A list of replica
endpoints on the same origin, as paths or URLs, can be given instead:
`?endpoints=/__mock/replicas/r1/api/v1/ws,/__mock/replicas/r2/api/v1/ws`;
the standby connection then uses another endpoint than the primary. An
endpoint on another origin stops the page with an error, so a link
cannot send the session id or the writes to another host. Every connection carries
the tab's session id as `?session=` (16 random bytes, base64url), the
same on the standby and on every reconnect; the mock refuses an upgrade
without one with 400. The session id leaves the tab only towards the
server: owner records carry its handle, the first 16 bytes of SHA-256
over it in base64url, which the client also computes for its own
session (`src/lib/net/handle.ts`) to recognise its own ownership.

## Client

The WebSockets run in a Web Worker (`src/lib/sync/worker.ts`) around
`SyncEngine` (`src/lib/sync/engine.ts`): a primary connection and a
standby on another replica, heartbeats both ways every 250 ms, failover
to the standby when the primary closes, misses three beats or reports
that it cannot commit (a stall of the worker itself is not counted as
missed beats: a watch timer that fires late first runs the messages
that arrived meanwhile), or gives no state it can apply within 3 s of a
hello, resync or promote, the command outbox, meter credits and decoding,
and the replicated state. The page only renders: it sends a tick per
animation frame while visible and applies the one patch the worker
answers with, leaf by leaf, measured as the `faderz:patch` performance
measure. A hidden page sends no ticks, so the worker grants no meter
credits. The state machines are in `src/lib/sync/machines.ts`.
Every snapshot and resume carries the cluster's epoch, the id of its
history; a cluster restored from a backup or started empty has a new one.
The client sends its epoch with its revision in hello, resync and
promote, and is resumed only within that epoch. A client whose state is
not in the cluster's history, a snapshot of another epoch or `ahead` (a
revision above the committed one in the same epoch), applies nothing:
the link shows AHEAD, a banner over the view names both revisions,
nothing is retried, and TAKE CLUSTER STATE loads the cluster's snapshot.

Every write control is `aria-disabled` and sends nothing while the link transmits no commands (connecting, offline, failing over, ahead), when the status area shows LOCK, and while the serving replica cannot commit and no standby is ready, when the client stays connected and the status area shows NO Q; either one names the reason in its title (`src/lib/state/writes.ts`). A command answered 503 (not applied), unanswered when a snapshot answers a hello or resync sent after it or answers none, or unanswered when its connection fails, is sent again with its id; otherwise one without an answer is not sent again, as the server answers every command it reads. One still unanswered 3 s after it was issued is dropped with a notice, and its control shows the committed value again.

Each screen latches true-peak overs and holds the maximum true peak
(the TP readout of a master) from the meter frames it receives, so a
screen opened later shows only what it measured since. RESET restarts
the loudness measurement of an output for every client; each screen
sees the restart in the frames (the maxima since reset fall, LRA turns
unstable) and clears that output's over and TP maximum, so all screens
agree. Channels latch an over only where they feed a direct out; the TP
key above the strips clears those on this screen only.

`window.__faderz` is a read-only view of the sync state for tests:
`connection`, `standby`, `primaryReplica`, `standbyReplica`, `session`
(dev builds only: the session id is a credential), `revision`,
`outbox`, `failovers`, `lastFailover`, `replicas`, `touching`,
`meterReplica`, `meterCredits`, `ticking`, `lastPatchMs`, `maxPatchMs`
and `patches`.

## Tests

```sh
npm run check      # svelte-check; type errors and warnings fail
npm test           # Vitest: units and contract tests
npm run test:e2e   # Playwright against the mock on ports 5181 to 5183
npm run build      # production bundle in dist/
```

The contract tests compile the schemas, resolve every `$ref` of
`openapi.yaml` and drive the mock over HTTP and WebSocket, checking
every message and response with an independent validator.

The e2e tests start three servers with `FADERZ_MOCK_AUTOMATION=0`:
dev servers on port 5181 (plain HTTP on all interfaces; the phone
projects open it through a LAN address) and 5182 (HTTPS with a
generated self-signed certificate), and on 5183 a production build
under `vite preview`, which serves the mock too
(`FADERZ_MOCK_CHANNELS=64`, for `perf.spec.ts`).
`FADERZ_E2E_PORT=<n>` moves the three servers to ports n, n + 1 and n + 2.
The specs share each server's mock state and run serially; each test
restores every replica and clears the command logs and counters.
`perf.spec.ts` and `throttle.spec.ts` run without tracing, whose DOM
snapshots run on the page's main thread.

## Keyboard

Faders are sliders: the arrows step 0.5 dB from -20 dB up, 1 dB down to
-40 dB and 3 dB below (Shift 0.1 dB), Page Up/Down 5 dB, Alt+0 sets
0 dB and Alt+End closes the fader; no single key jumps a fader, and a
double-click does nothing to it. Pan controls step with the arrows
(Shift for fine), Alt+Home and Alt+End go to the ends and Alt+0
centres; no single key and no double-click moves a pan. Escape
closes the channel processing panel. The mute of an output (bus, aux,
mix-minus) needs a 0.5 s hold, by pointer, Space or Enter; one press
unmutes. Channel mutes take one press.

TALK on a mix-minus output, by pointer, Space or Enter: held longer
than 0.3 s it talks until released; a shorter tap latches it on. Only
a release on the key latches: a cancelled touch, a lost pointer or
focus leaving the key during the press ends the talk, and so does
leaving the view, or the page going hidden or away, while the key is
held or latched. A press on a lit
key stops the talk, whichever screen lit it; one another session holds
is stopped there, and the screen names the holder. A latch stopped
while writes are locked stays shown until the server reports the talk
off.

A label cut off for lack of room, such as a long channel name or the
status-bar counters, shows its full text in a tooltip: after a short
hover, on keyboard focus (Tab to it) and on a touch held for 0.5 s;
Escape closes it.

Typed values (the readout of every processing parameter, delay, the
Setup view) are written on Enter or when the field is left, and Escape
restores the current value. Parameters take units: `2.5k` or `2.5 kHz`,
`-3 dB`, `40%`, `4:1`, `off`. A delay is typed
in the selected unit (ms, frames at the shape's frame rate, or
samples) or with a suffix: `40ms`, `2f`, `1.5s`, `1920smp`; the arrow
keys step one unit, Shift a tenth. Matrix grids move with the arrow keys.

With a pointer: drag a fader, hold Shift (or Ctrl) for fine travel.
