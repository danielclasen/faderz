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
loudness, AFV tally cuts every 8 s and console activity (a fader ride,
bank changes, and every duck group held for 4 s in every 8 s), and validates
every message it sends and receives against the contract; each meter
frame is decoded and its values validated before it is sent. A duck
group with a trigger and a target keys from the speech of its triggers,
with the detector, hold and envelope its settings give.

- `FADERZ_MOCK_AUTOMATION=0` turns off the tally cuts and console
  activity, so nothing changes state but clients and REST calls, and
  keys every duck group in use at a steady -50 dBFS, so a group ducks
  only while held or on, or with its threshold below -50 dBFS.
- `FADERZ_MOCK_CHANNELS=<n>` sets the input channel count (default 29):
  mono channels `ch1`... followed by up to four stereo channels and one
  5.1 channel.
- `FADERZ_BACKEND=<url>`, such as `http://127.0.0.1:8080`, loads no
  mock: `/api/v1`, WebSocket included, goes to that faderz server.

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
| `POST /__mock/slow-link?ms=<n>` | answers every later REST request and WebSocket upgrade `n` ms late, as over a slow link; `ms=0` ends it |
| `POST /__mock/consoles/<id>/bank?offset=<n>` | moves that console's bank to start at channel index `n` |
| `POST /__mock/ducks/script?ms=<n>` | a console session holds the hold of every duck group for `n` ms, lets go for `n` ms, and so on; `ms=0` stops and releases |
| `POST /__mock/skip-revision` | advances the revision without a delta |
| `POST /__mock/write` | body `{"changes": [{path, value}]}`: commits the changes in one revision whether or not a replica can commit, as a restore writing state does; answers like `PATCH /state` |
| `POST /__mock/rewind?by=<n>` | rolls the cluster back `n` revisions, as a restore from an older backup would: a new epoch and every socket closed, so a client that held state reconnects with the old epoch and gets a snapshot of the new one |

A replica that is not partitioned itself but is left without a majority
of ready, unpartitioned replicas, for example the third one while the
other two are partitioned, reports `commitable: false` as well and
answers every command with error 503; no replica can commit then.

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
that it cannot commit while the standby reports that it can, and still
does 250 ms later (a stall of the worker itself is not counted as
missed beats: a watch timer that fires late first runs the messages
that arrived meanwhile), or gives no state it can apply within 3 s of a
hello, resync or promote, the command outbox, meter credits and decoding,
and the replicated state. While neither its primary nor its standby can
commit, the client lets the standby go to look for a third replica when
a report names one that can, and fails over to it; while no replica can
commit it stays on its primary and keeps its standby, so nothing fails
over or reconnects. The page only renders: it sends a tick per
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

Every write control is `aria-disabled` and sends nothing while the link transmits no commands (connecting, offline, failing over, ahead), when the status area shows LOCK and names the reason in its title (`src/lib/state/writes.ts`). While the serving replica cannot commit, the status area shows NO Q and the controls stay usable: the notice line says CHANGES NOT COMMITTING - audio continues, with the number of writes pending, and every key pressed meanwhile shows its state desaturated inside a dashed edge until its write commits or expires (`src/lib/pending.ts`, which every key that writes uses). Notices raised meanwhile wait and are timed once the line shows them again. REST writes are answered 503 then and change nothing. TALK starts no talk then, and a press on a latched TALK sends only the release, so the key stays latched until the state says the talk is off; a lit TALK never looks disabled. A command answered 503 (not applied), unanswered when a snapshot answers a hello or resync sent after it or answers none, or unanswered when its connection fails, is sent again with its id; otherwise one without an answer is not sent again, as the server answers every command it reads. One still unanswered 3 s after it was issued is dropped with a notice, and its control shows the committed value again.
The LOCK DESK key in the header does the same on one screen: a 0.5 s
hold locks it, another unlocks it. A phone shows the key only while
the desk is locked.

Each screen latches true-peak overs and holds the maximum true peak
(the TP readout of a master) from the meter frames it receives, so a
screen opened later shows only what it measured since. RESET restarts
the loudness measurement of an output for every client; each screen
sees the restart in the frames (the maxima since reset fall, LRA turns
unstable) and clears that output's over and TP maximum, so all screens
agree. PAUSE pauses an output's integration after a 0.5 s hold; paused,
the key is lit amber and reads PAUSED, one press runs it on, and the
held integrated loudness, LRA and maxima carry a dashed amber edge on
every screen. Momentary, short-term and true peak go on while paused,
the TP maximum included; no paused audio enters the integration. What a
hold does is decided when it starts. A scene recall leaves the
integration paused or running, and RESET leaves it paused. Channels latch an over only where they feed a direct out; the TP
key above the strips clears those on this screen only.

### Sign-in

At start the page reads `GET /api/v1/session` (`src/lib/net/auth.ts`).
Not signed in, in `oidc` mode it goes to `/api/v1/login?return=<path>`,
which sends the browser to the identity provider and back; if it went
there within the last 30 s it shows a prompt with a SIGN IN link
instead, so a sign-in that does not take cannot loop. In `token` mode
it shows a prompt for the API token, which it exchanges with
`POST /api/v1/login` for a session cookie. When the link leaves Live and
while it is down the
page reads the session again (then every 5 s), so an ended session brings the
prompt back, or in `oidc` mode goes to the sign-in again (within the
same 30 s guard); a REST request answered 401 reads the session again
too. The status bar names the user and role, and in `token` and
`oidc` mode offers Sign out (`POST /api/v1/logout`), which in `oidc`
mode continues to the provider's end-session endpoint when it has one.
A viewer's write controls are locked with the reason "signed in as a
viewer"; the server refuses a viewer's writes either way.

`oidc` mode takes these settings, each in the configuration file, as a
flag of `faderz` and as an environment variable; the flag wins over the
variable and the variable over the file (`faderz --help`):

| Key under `auth.oidc` | Flag | Variable | Default |
| --- | --- | --- | --- |
| `issuerUrl` | `--oidc-issuer-url` | `FADERZ_OIDC_ISSUER_URL` | required |
| `clientId` | `--oidc-client-id` | `FADERZ_OIDC_CLIENT_ID` | required |
| `clientSecret.file` | `--oidc-client-secret-file` | `FADERZ_OIDC_CLIENT_SECRET_FILE` | none: a public client |
| `publicUrl` | `--oidc-public-url` | `FADERZ_OIDC_PUBLIC_URL` | required |
| `roleClaim` | `--oidc-role-claim` | `FADERZ_OIDC_ROLE_CLAIM` | `groups` |
| `scopes` | `--oidc-scopes` | `FADERZ_OIDC_SCOPES` | none besides `openid` |
| `operatorRole` | `--oidc-operator-role` | `FADERZ_OIDC_OPERATOR_ROLE` | `faderz-operator` |
| `viewerRole` | `--oidc-viewer-role` | `FADERZ_OIDC_VIEWER_ROLE` | none |
| `allowedClients` | `--oidc-allowed-clients` | `FADERZ_OIDC_ALLOWED_CLIENTS` | the client id |
| `audience` | `--oidc-audience` | `FADERZ_OIDC_AUDIENCE` | none: not checked |
| `sessionTtl` | `--oidc-session-ttl` | `FADERZ_OIDC_SESSION_TTL` | `8h` |
| `sessionKey.file` | `--oidc-session-key-file` | `FADERZ_OIDC_SESSION_KEY_FILE` | drawn per process |

`auth.mode` is `--auth-mode` and `FADERZ_AUTH_MODE`. The redirect URI to
register with the provider is `<publicUrl>/api/v1/login/callback`, and
`<publicUrl>/` is the post-logout redirect URI. A user whose `roleClaim`
holds `operatorRole` is an operator; any other user is a viewer, or,
with `viewerRole` set, a viewer only when the claim holds that and
refused otherwise. A value matches with or without a leading `/`, as
group paths are written, and a `roleClaim` with `.` that is no
top-level claim is followed as a path (`realm_access.roles`). The role is
the one at sign-in; a change at the provider applies from the next one.
A session lasts `sessionTtl` from the sign-in (seconds, or a number with
`s`, `m`, `h` or `d`, 60 s to 7 d), whatever the ID token's lifetime.
Signing out ends it on the replica that answers, for every copy of the
cookie, and closes the WebSockets opened with it there; that replica
holds the sign-out in memory, so another replica, or the same one after
a restart, takes a copy of the cookie until it expires; WebSockets are
also closed when it expires. `issuerUrl` and every endpoint the provider
publishes need `https`, or `http` to a loopback address, as does
`publicUrl`, and `sessionKey` needs 32 bytes or more. Without
`sessionKey` a session holds on the process that signed it and ends when
that restarts.

API clients send the `auth.token` when one is configured, or an access
token of the provider: not an ID token (no `nonce`, no `typ` `ID`), with
an `azp` among `allowedClients`, and an `aud` holding `audience` when that
is set. Keycloak puts `account` into an access token's `aud` and the
client id only through an audience mapper; with `audience` unset, `azp`
alone binds the token to the allowed clients. faderz logs at start
which auth mode applies and whether the file, a flag or a variable set
it.

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

### Against the faderz backend

```sh
FADERZ_BIN=../target/debug/faderz npm run test:backend
FADERZ_E2E_BACKEND=../target/debug/faderz npm run test:e2e
```

`test:backend` starts the binary on port 6461 (`FADERZ_BACKEND_PORT`
moves it, and its ops and NMOS listeners take the two after it) and
checks its REST responses against `openapi.yaml` and its WebSocket
messages against the schemas, with the validators of the contract
tests. `FADERZ_E2E_BACKEND` runs the e2e suite against one replica of
the binary (API, ops and NMOS on ports n + 3 to n + 5) behind the two
dev servers, with the mock's example shape; a global setup writes the
mock's example state over REST. Tests tagged `@mock` need the mock's
controls or its replicas, `@meters` its meter frames; they are left out
of that run, and so is the production build server.

### The UI from the binary

The faderz binary carries the UI it is built with: `npm run build` here
before `cargo build` at the repository root. A build without `dist/`
answers `/` with 404 and a note saying so, and `cargo test` then checks
only that answer.

```sh
FADERZ_BIN=../target/debug/faderz npm run test:binary
```

starts the binary four times, on ports 6471, 6481, 6491 and 6501
(`FADERZ_BINARY_PORT` moves the first; ops and NMOS take the two after
each), in auth modes `none`, `token`, `proxy` and `oidc`: `proxy` behind
a stand-in authenticating proxy on 6495 (`tests/binary/proxy.mjs`) that
drops identity headers a client sends and sets its own, `oidc` against
a stand-in OpenID provider on 6505 (`tests/binary/idp.mjs`) that signs
alice in as an operator, with its settings given as flags and
environment variables. Each mode loads
the UI from the binary and writes through it, within the binary's
Content-Security-Policy; tests tagged with a mode check what only that
mode refuses. In `proxy` mode the binary takes who the identity headers
name, so it refuses to start with its API listener bound anywhere but
loopback, where only the proxy beside it reaches it.

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
held or latched; back on the page, a notice says the talk ended and
why. A press on a lit
key stops the talk, whichever screen lit it; one another session holds
is stopped there, and the screen names the holder. A latch stopped
while writes are locked stays shown until the server reports the talk
off.

DUCK on a duck group's tile in the masters, or on its pill in the bank
bar between 1024 and 1399 px, where the masters lie over the strips,
by pointer, Space or Enter: the press holds the duck in at once, and
held longer than 0.3 s it is momentary; a shorter tap latches mode on,
or sets a latched group back to auto. Shift+D+n holds the n-th group
while the digit is down, in every view, and never latches; with a key
of the group held as well, the hold ends when the last one lets go. A cancelled touch, a lost
pointer, focus leaving the key, the page going hidden or away, or
leaving the view ends a hold and changes no mode; the latch stays, on
every screen, until mode is set again. While another session holds a
group the press holds nothing, the notice names the holder, and the
key's face shows it; a tap still latches, as mode has no owner. On a
bypassed group (mode OFF, hatched) a tap sends nothing and says so, and
a press holds the duck in only once it has lasted 0.3 s. OFF takes a
0.5 s hold, AUTO and ON one press; the mode keys are one tab stop, and
the arrow keys move the focus among them without selecting. No hold
starts while nothing commits. The tile says what the group does: while
its envelope is above 0, how far its deepest target is lowered (DUCK
-12.0), else DUCK HELD, DUCK ON, BYPASS or AUTO, with KEY while the key
triggers it; it sets the threshold, and its key meter row shows the key
level against it. More groups than fit one row of tiles scroll within
it. Beside the masters with a fine pointer, where the tiles leave the
band at least 180 px, the band's summary takes the rest of it: a line
per group with its name, the buses, auxes and mix-minus outputs it
lowers, the targets it lowers in them, its gain reduction as a bar on
one scale for every group (0 to the deepest depth of any) and in dB,
and its key as a bar against its threshold, KEY lit while the key
triggers it. Narrower than 300 px the line keeps the name, the outputs,
the reduction in dB and the key bar, coloured while the key triggers;
the band keeps its height. In the example shape's masters, 776 px
wide, that is the full line up to three groups, the condensed one at
four and none from five. Under 1400 px, where the masters lie over the
strips, and with a coarse pointer the band shows its tiles alone. A
strip in a group carries a DK chip (KEY on a trigger), lit while
a group that lowers it is held or latched in or lowers it now, when it
reads the gain reduction in place of DK, or, on a trigger, while a
group it keys is engaged; it opens the Duck section of its processing
panel. The
DUCK view sets up every group: mode, key tap, detector and key filters,
destinations, and per channel TRIG, TARGET and the depth; up to 1280 px
the detector starts folded to its mode keys. Its channel grid is one
tab stop, walked with the arrow keys. The presets LIGHT, HEAVY and
VOICE write in one atomic PATCH and take a second press within 3 s or a
0.5 s hold; leaving the key disarms it. A phone shows one group at a
time.

A gesture on a control another session holds is refused at once and
the notice names the holder. ASK in that notice sends the holder a
request for the control, at most once per control every 3 s (the
cluster drops more frequent ones), and once it is sent the notice line
says so. The holder's screen shows the request as a notice with
RELEASE, which ends a talk or releases any other control; nothing
changes until the holder lets go or its lease runs out, and its lease
runs on while any of its connections is open. The head of a strip that another screen
has selected carries that screen's initial, the first letter of its
handle tag, and the head's tooltip names every screen that has it
selected. The cluster sends selections at most every 100 ms, and the
client shows those of its primary connection.

USERS counts the sessions connected to the primary connection's
replica, this screen included, the connected control surfaces, and
the sessions on other replicas that hold a control. Its panel lists
each with who is signed in (the display name, or token or anonymous),
the role, the handle tag, what it holds and what it has selected;
every role sees it. Each replica knows only its own connections.

A label cut off for lack of room, such as a long channel name, shows
its full text in a tooltip: after a short hover, on keyboard focus (Tab
to it) and on a touch held for 0.5 s; Escape closes it.

In the mixer, `[` and `]` step to the previous and next bank, the Jump
field takes a strip number or the start of a channel name, FOLLOW keeps
the strips on the first connected surface's bank, and a press on a VCA
master's name shows only that VCA's members until EXIT. "Sends on
faders" puts one aux on the channel faders: each fader then sets that
channel's send level to the aux, under a band in the sends colour,
until SOF ... - EXIT.

A strip has five tab stops: its head (select), PFL, AFL, MUTE and the
fader, and after them its VCA row while it is in a VCA, whose tooltip
names the VCAs and the effective gain; its chips and pan are edited in
the processing panel. The first
tab stops of the mixer are the skip links to the masters and the
monitor.

Typed values (the readout of every processing parameter, delay, the
Setup view) are written on Enter or when the field is left, and Escape
restores the current value. Parameters take units: `2.5k` or `2.5 kHz`,
`-3 dB`, `40%`, `4:1`, `off`. A delay is typed
in the selected unit (ms, frames at the shape's frame rate, or
samples) or with a suffix: `40ms`, `2f`, `1.5s`, `1920smp`; the arrow
keys step one unit, Shift a tenth. Matrix grids move with the arrow keys.
The slider of a parameter or a send level moves by the distance dragged,
from where it was, and never jumps to the touch point; its arrows step a
hundredth of its travel, Page Up and Page Down a tenth, and every press
moves the value by at least one step of its resolution.

With a pointer: drag a fader, hold Shift (or Ctrl) for fine travel; the
FINE key in the bank bar latches fine travel for faders and pans, for
touch screens. A double-click does nothing to a fader.
