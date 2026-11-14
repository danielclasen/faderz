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
| `POST /__mock/replicas/<id>/partition` | reports `commitable: false`, ignores commands, renews no lease |
| `POST /__mock/replicas/<id>/heal` | ends the partition |
| `POST /__mock/replicas/<id>/lag?ms=<n>` | log entries reach the replica `n` ms late; it answers a hello or resync carrying `lastRevision`, and runs commands after a promote, only once it has applied that revision; `ms=0` catches up |
| `POST /__mock/blocks/hold` | keeps the current processing block open, so every command joins one log entry |
| `POST /__mock/blocks/flush` | ends the hold and commits the open block |
| `GET /__mock/stats` | per live connection: `connection, session, kind, mode, replica, meterFramesSent, metersSkippedNoCredit, creditsOutstanding, messagesIn, messagesOut, openedAt`; `totals` over all connections since start or reset |
| `POST /__mock/stats/reset` | zeroes the counters |
| `GET /__mock/audit` | applied WebSocket commands in apply order, `{owner, commandId, type, seq, path, value, revision, replica}`, and `summary: {total, duplicates}`, where `duplicates` counts command ids applied more than once |
| `GET /__mock/commands` | every command outcome, REST included |
| `POST /__mock/audit/reset` | clears both command logs |
| `POST /__mock/drop-clients` | closes every socket |
| `POST /__mock/skip-revision` | advances the revision without a delta |
| `POST /__mock/rewind?by=<n>` | rolls the cluster back `n` revisions, as a restore from an older backup would; a client holding a later revision is answered with `ahead` |

`/__mock/replicas/<id>/api/v1/ws` connects to one replica;
`/api/v1/ws` picks a serving replica at random.

HTTPS for the dev server: set `FADERZ_DEV_TLS_CERT` and `FADERZ_DEV_TLS_KEY`
to PEM files, for example a self-signed pair; the UI then uses `wss://`.
Plain HTTP works too, also on LAN addresses that are not secure
contexts.

The UI connects to `/api/v1/ws` on its own origin. A list of replica
endpoints can be given instead:
`?endpoints=ws://a/api/v1/ws,ws://b/api/v1/ws`; the standby connection
then uses another endpoint than the primary. Every connection carries
the tab's session id as `?session=` (16 random bytes, base64url), the
same on the standby and on every reconnect; the mock refuses an upgrade
without one with 400.

## Client

The WebSockets run in a Web Worker (`src/lib/sync/worker.ts`) around
`SyncEngine` (`src/lib/sync/engine.ts`): a primary connection and a
standby on another replica, heartbeats both ways every 250 ms, failover
to the standby when the primary closes, misses three beats or reports
that it cannot commit, the command outbox, meter credits and decoding,
and the replicated state. The page only renders: it sends a tick per
animation frame while visible and applies the one patch the worker
answers with, leaf by leaf, measured as the `faderz:patch` performance
measure. A hidden page sends no ticks, so the worker grants no meter
credits. The state machines are in `src/lib/sync/machines.ts`.
A client holding a revision the cluster never committed (a cluster
restored from an older backup) gets `ahead` instead of state: the link
shows AHEAD, a banner over the view names both revisions, nothing is
retried, and TAKE CLUSTER STATE loads the cluster's snapshot.

When the serving replica cannot commit and no standby is ready, the client stays connected, the status area shows NO Q, and every write control is `aria-disabled` and sends nothing until quorum returns (`src/lib/state/writes.ts`).

`window.__faderz` is a read-only view of the sync state for tests:
`connection`, `standby`, `primaryReplica`, `standbyReplica`, `session`,
`revision`, `outbox`, `failovers`, `lastFailover`, `replicas`,
`touching`, `meterReplica`, `meterCredits`, `ticking`, `lastPatchMs`,
`maxPatchMs` and `patches`.

## Tests

```sh
npm run check      # svelte-check; type errors and warnings fail
npm test           # Vitest: units and contract tests
npm run test:e2e   # Playwright against the mock on port 5181
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
The specs share each server's mock state and run serially; each test
restores every replica and clears the command logs and counters.
`perf.spec.ts` and `throttle.spec.ts` run without tracing, whose DOM
snapshots run on the page's main thread.

## Keyboard

Faders are sliders: the arrows step 0.5 dB from -20 dB up, 1 dB down to
-40 dB and 3 dB below (Shift 0.1 dB), Page Up/Down 5 dB, Alt+0 sets
0 dB and Alt+End closes the fader; no single key jumps a fader, and a
double-click does nothing to it. Pan controls step with the arrows,
Home and End go to the ends, `0` and a double-click centre. Escape
closes the channel processing panel.

Typed values (delay, the Setup view) are written on Enter or when the
field is left, and Escape restores the current value. A delay is typed
in the selected unit (ms, frames at the shape's frame rate, or
samples) or with a suffix: `40ms`, `2f`, `1.5s`, `1920smp`; the arrow
keys step one unit, Shift a tenth. Matrix grids move with the arrow keys.

With a pointer: drag a fader, hold Shift (or Ctrl) for fine travel,
double-click for 0 dB.
