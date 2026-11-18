# faderz

An operated multichannel broadcast audio mixer over MXL, built on the
[samplez](https://github.com/danielclasen/samplez) engine and written in
Rust, with a web console in Svelte.

faderz reads 48 kHz audio flows from MXL domains, mixes them under a
replicated mixer state and writes every configured output to its own MXL
flow. The mixer's shape (channels, buses, auxes, mix-minus outputs, VCAs,
monitor and output flows) is fixed in one JSON configuration file; the
state is changed through a REST API and a WebSocket, which the web console
built into the binary and the control-surface bridge both speak.

![The mixer view of a music and talk show: channel strips with tally, processing chips, faders and meters, VCA masters, programme, M&E and clean-feed masters with loudness readouts, and the monitor section](docs/screenshots/console.png)

## Features

- Channels of mono, stereo, 5.1 or 7.1 layout, each with an A/B input
  select, trim and leg trim, polarity, high-pass filter, four-band EQ,
  gate and compressor before or after the EQ, pan, a scribble-strip
  colour, PFL and AFL, a meter tap and a direct out
- Programme, M&E and clean-feed buses with routing per channel, auxes
  with pre or post sends, mix-minus outputs that leave chosen channels
  out of a bus, and VCAs with offsets and mutes
- A monitor section with source select, level, dim, cut, mono, Lo/Ro or
  L/R fold-down and per-speaker mutes and solos
- Peak, true-peak and loudness metering (momentary, short-term,
  integrated, LRA) against an EBU R 128, ATSC A/85 or custom target,
  streamed as binary meter frames
- Scenes with recall-safe parameter groups per element
- Audio follows video: tally from a vision switcher over REST
  (`/api/v1/afv/tally`) opens a channel or lowers it to its off level
- Several operators at once: every control a session touches is held by
  that session until it lets go, others see who holds it and can ask for
  it, and the USERS panel lists who is connected and what they hold and
  have selected
- Auth modes `none`, `token`, `proxy` (behind an authenticating proxy) and
  `oidc` (OpenID Connect sign-in in faderz itself), with operator and
  viewer roles
- A replicated state: one replica on a data directory, or a set of an odd
  number of voters with a WebSocket standby on another replica and
  failover between them
- Hardware control surfaces through `faderz-bridge` (Mackie Control over
  USB-MIDI or RTP-MIDI)
- `/healthz`, `/readyz` and Prometheus `/metrics`; a container image and
  a release binary

The engine applies the channel strip (trim, leg trim, polarity,
high-pass, EQ, gate, compressor), faders, mutes, VCA offsets and mutes,
AFV open and closed with the off level, mono pan and the fold-down of
wider channels, bus routing, sends, the bus, aux and mix-minus faders and
mutes, bus and aux polarity, mix-minus exclusions, and the monitor's
source, fold-down, level, dim, cut, mono and speaker mutes and solos. The
rest of the state is kept, shown and replicated but not applied to the
audio in this version: channel delay, PFL and AFL listening, VCA solo,
front/rear pan, divergence and LFE send, the stereo image setting of a
channel and of the monitor, the monitor's talkback dim, bus and aux
processing (EQ, dynamics, limiter, delay), the line-up tone, mix-minus
talkback interrupts, the AFV rise, hold and fall times, duck groups, and
the scene crossfade time.

## The console

The web console has six views: MIXER, ROUTING, AUX / N-1, AFV, SCENES and
SETUP. It runs in a desktop browser, on a tablet and on a phone, in a
dark or a light theme. `web/README.md` documents it in detail: its
connection handling, sign-in, keyboard control and tests.

Selecting a strip opens its processing panel: input colour, trim,
polarity, high-pass, delay, meter and direct-out taps, pan, the EQ with
its curve, gate and compressor.

![The processing panel of the Lead Vox channel with input settings, the EQ curve and four bands, gate and compressor](docs/screenshots/channel-processing.png)

Auxes and mix-minus outputs have their own masters with loudness meters,
a talkback source per mix-minus, the send matrix and the mix-minus
exclusions.

![The AUX / N-1 view: IFB, stage, record and N-1 masters, the aux sends of every channel and the channels left out of each mix-minus](docs/screenshots/aux-mix-minus.png)

The AFV view shows the tally the vision switcher reports and which
channels follow which camera.

![The AFV view with CAM1 on programme and CAM2 on preview, and the channels following them](docs/screenshots/afv-tally.png)

Scenes are stored and recalled from the SCENES view; recall-safe groups
keep chosen parameters of a channel out of every recall. The view's note
on crossfading describes the contract's recall; the engine takes a recall
at once, without the crossfade.

![The SCENES view with six scenes, the current one marked, and the recall-safe grid](docs/screenshots/scenes.png)

Every screen is a session. The USERS panel lists the screens and
surfaces connected to the replica, with what each holds and has
selected. The ENGINE lamp in the status bar opens the engine's counters
of the replica serving the screen and shows WARN while one of them has
grown in the last 10 s.

![The USERS panel listing three operators, one holding the Guest 2 fader and one with Lead Vox selected, and two connected control surfaces](docs/screenshots/users.png)

![The engine counters: deadline misses, overrun samples, late input blocks, input wait timeouts, late commands and late AFV changes](docs/screenshots/engine-counters.png)

Below 1400 px of width the strips take the full width and the MASTERS
key shows the masters over them.

![The mixer on a tablet in the light theme](docs/screenshots/tablet-light.png)

The screenshots are taken from the UI's dev mock with invented show data
(`web/tests/screenshots/`). The mock's meters carry a correlation value
(COR), which the engine does not measure: against faderz it reads
`--.--`.

## Quick start

This builds the image from this repository and starts it with a small
shape and no `mxl` section: the state, the API and the console are
served, no engine runs and nothing is mixed. Releases publish the same
image as `ghcr.io/danielclasen/faderz:<version>`. The configuration and
the data directory go to a directory of their own, outside the build
context.

```sh
docker build -t faderz:local .
cd "$(mktemp -d)"
mkdir -p data
cat > config.json <<'JSON'
{
  "listeners": {
    "api": { "bind": "0.0.0.0", "port": 8080 },
    "ops": { "bind": "0.0.0.0", "port": 9090 }
  },
  "replica": { "name": "studio" },
  "shape": {
    "sampleRate": 48000,
    "frameRate": { "numerator": 25, "denominator": 1 },
    "channels": [
      { "id": "host", "name": "Host", "width": 1, "layout": "mono", "altInput": false },
      { "id": "guest", "name": "Guest", "width": 1, "layout": "mono", "altInput": false },
      { "id": "band", "name": "Band", "width": 2, "layout": "stereo", "altInput": false }
    ],
    "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
    "auxes": [{ "id": "ifb", "name": "IFB", "width": 1, "layout": "mono" }],
    "mixMinus": [],
    "vcas": [{ "id": "talent", "name": "TALENT" }],
    "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
    "outputs": [{ "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" }]
  }
}
JSON
docker run --rm --name faderz --user "$(id -u):$(id -g)" --read-only --tmpfs /tmp \
  -v "$PWD/config.json:/etc/faderz/config.json:ro" -v "$PWD/data:/var/lib/faderz" \
  -p 127.0.0.1:8080:8080 -p 127.0.0.1:9090:9090 \
  faderz:local
```

Open <http://127.0.0.1:8080/>, or in another shell:

```sh
curl -s http://127.0.0.1:8080/api/v1/state
curl -s http://127.0.0.1:9090/metrics
```

The state is kept in `data/` and survives a restart. Without an `mxl`
section `/healthz` and `/readyz` answer 503 (see [Health](#health)).
With `auth.mode` `none` the API answers only requests whose `Host` is
`localhost`, `127.0.0.1` or listed in `api.allowedHosts`, and takes
WebSocket upgrades only from the origins `http://localhost:<api port>`
and `http://127.0.0.1:<api port>` or those in `api.allowedOrigins`, so
the host port above is the container's.

### With audio

The `mxl` section names the libmxl domains and the flow each channel
reads and each output writes; every channel and every output needs one.
An MXL domain is a directory on tmpfs shared with the processes that
write and read its flows; here `/dev/shm/mxl/studio`, writable by the
image's user, uid 65532.

```json
"mxl": {
  "domains": [{ "name": "studio", "path": "/mxl/studio" }],
  "inputs": {
    "host": { "flow": "00000000-0000-4000-8000-000000000001" },
    "guest": { "flow": "00000000-0000-4000-8000-000000000002" },
    "band": { "flow": "00000000-0000-4000-8000-000000000003" }
  },
  "outputs": {
    "out-pgm": { "flow": "00000000-0000-4000-8000-000000000010" }
  }
}
```

```sh
docker run --rm --name faderz --read-only --tmpfs /tmp \
  --cap-drop ALL --cap-add SYS_NICE --cap-add IPC_LOCK \
  -v /dev/shm/mxl/studio:/mxl/studio \
  -v "$PWD/config.json:/etc/faderz/config.json:ro" -v faderz-data:/var/lib/faderz \
  -p 127.0.0.1:8080:8080 -p 127.0.0.1:9090:9090 \
  faderz:local
```

The image runs as uid 65532. `SYS_NICE` and `IPC_LOCK` give the engine's
real-time thread `SCHED_FIFO` and locked memory; without them it starts
degraded and logs so (`faderz: rt ...`).

## Configuration

One JSON file, `faderz --config <file>`. Unknown keys are refused,
except under `engine` and `rt`, which are checked only when an `mxl`
section is present. `faderz --help` lists the flags. Each `auth` setting
below with a flag can also be given as that flag or as its environment
variable; the flag wins over the variable, the variable over the file.

| Key | Default | |
|---|---|---|
| `listeners.api` | `127.0.0.1:8080` | `{bind, port, tls}`: `/api/v1`, `/api/v1/ws` and the console |
| `listeners.ops` | `0.0.0.0:9090` | `{bind, port, tls}`: `/healthz`, `/readyz`, `/metrics` |
| `listeners.machine` | none | `{bind, port, tls}`: a bearer-only listener for machine clients; `proxy` mode only |
| `listeners.nmos` | `127.0.0.1:8081` | `{bind, port, tls}`: bound at start; faderz serves no routes on it |
| `listeners.*.tls` | `{"mode": "none"}` | `none`; `files` with `certFile`, `keyFile` and optional `clientCaFile`, re-read when they change; `selfSigned` with `sans`, generated at start |
| `auth.mode` | `none` | `none`, `token`, `proxy` or `oidc`; `--auth-mode`, `FADERZ_AUTH_MODE` |
| `auth.token.file` | | the API bearer token's file; required in `token` mode, optional in `oidc` mode |
| `auth.machineToken.file` | | the machine listener's bearer token; `proxy` mode |
| `auth.proxy.userHeader` | `X-Forwarded-User` | `proxy` mode: the header naming the user |
| `auth.proxy.groupsHeader` | `X-Forwarded-Groups` | `proxy` mode: comma-separated groups |
| `auth.proxy.groupRoles` | `{}` | `proxy` mode: group name to `operator` or `viewer` |
| `auth.oidc.*` | | `oidc` mode; the keys, flags and variables are in `web/README.md` |
| `api.allowedHosts` | `[]` | `Host` names accepted on requests without a bearer token, besides `localhost` and `127.0.0.1` |
| `api.allowedOrigins` | `[]` | `Origin` values accepted on WebSocket upgrades, besides `localhost` and `127.0.0.1` on the API port |
| `replica.name` | the host name | the replica's id towards clients |
| `replica.dataDir` | `/var/lib/faderz` | the replicated log and snapshots |
| `replica.replicas` | `1` | the voters of the replica set, an odd count |
| `replica.ordinal` | from the name | this replica's ordinal in a set |
| `replica.peers` | | every replica's peer address, `host:port`, by ordinal |
| `replica.peer` | | `{bind, port, tls}`: the peer listener of a set |
| `replica.marker` | | the ConfigMap a set starts from; `FADERZ_REPLICA_MARKER` when the file sets none |
| `replica.maxApplyLeadMs` | `50` | 30 to 120: the most a set's apply deadline leads the engine; a set runs no engine yet, so it has no effect |
| `shape` | required | the mixer's shape, model `Shape` in `api/ws/model.schema.json` |
| `consoles` | `[]` | consoles known before any registers: `id`, `name`, `protocol` (`mcu`), `transport` (`bridge`), `strips` (8) |
| `mxl.libraryPath` | `/opt/mxl/lib/libmxl.so` | libmxl, loaded at start |
| `mxl.domains` | | `[{name, path}]`: the mounted MXL domains |
| `mxl.inputs.<channel>` | | `flow`; `altFlow` for a channel with `altInput`; `domain` with more than one domain; `channels`, the 1-based flow channel each leg reads |
| `mxl.outputs.<output>` | | `flow`; `domain` with more than one domain |
| `engine.sampleRate` | `48000` | |
| `engine.blockSize` | `48` | samples per block |
| `engine.readerMarginSamples` | `1536` | read delay behind libmxl's clock; delays every output by as much |
| `engine.maxCatchupBlocks` | `2` | overdue blocks processed in order after a late wake |
| `engine.pruneAfterBlocks` | `50` | blocks of a frozen producer head before an input is pruned |
| `engine.rampSamples` | `480` | the length of every gain ramp |
| `rt.enabled` | `true` | `false` skips capabilities, memory locking and `SCHED_FIFO` |
| `rt.priority` | `70` | `SCHED_FIFO` priority |
| `rt.rtCpu` | `auto` | `auto`, `none`, `first`, `last` or an index into the allowed CPUs |

Without an `mxl` section faderz runs no engine: the state is served,
nothing is mixed and `/readyz` fails. In `proxy` mode faderz takes the
user the identity headers name, so it refuses to start unless
`listeners.api.bind` is a loopback address, where only the proxy beside
it reaches it.

### A replica set

With `replica.replicas` above 1, faderz runs as one pod of a
StatefulSet per voter, the pods reaching each other on the peer
listener:

- Leave `replica.name` and `replica.ordinal` out: each pod takes its pod
  name and the ordinal that name ends in. A `name` given in a set needs
  an `ordinal`.
- `replica.peers` names every pod's peer address by ordinal, as many as
  `replicas`, each once: `<pod>.<headless service>:<port>`.
- `replica.peer` needs `tls.mode` `files` with a `clientCaFile` that has
  issued every replica's certificate; each certificate names the host of
  its `peers` address, which the others check. Port 0 is refused.
- `replica.marker` (or `FADERZ_REPLICA_MARKER`) names a ConfigMap in the
  pod's namespace: ordinal 0 creates it, the others read it to join, so
  the pods' service account needs to get and create ConfigMaps there.
- Ordinal 0 starts a set only while a majority of the voters answer, so
  the StatefulSet starts its pods in parallel
  (`podManagementPolicy: Parallel`).
- A set starts on empty volumes. The data directory of a set of one
  cannot join a set, nor that of a set run alone, so changing between
  the two needs new volumes and leaves the state behind.
- A set of more than one replica runs no engine: a configuration with an
  `mxl` section is refused. samplez's replicated engine takes one gain per
  log entry, and one mixer command sets many.
- In `oidc` mode the replicas behind one host name share
  `auth.oidc.sessionKey`.
- A set runs no engine, so its probes differ from a set of one; see
  [Health](#health).

## API

The contract is in `api/`: `openapi.yaml` for REST under `/api/v1` and
`ws/*.schema.json` for the WebSocket at `/api/v1/ws`. Writes carry a
client-generated command id that is applied at most once across all
replicas; a write is answered once the log entry carrying it has
committed.

| Method and path | |
|---|---|
| `GET /api/v1/state` | shape, live state, owners and the revision |
| `PATCH /api/v1/state` | write one or more controls atomically |
| `GET /api/v1/replicas` | the replica set as the serving replica sees it |
| `POST /api/v1/scenes`, `PUT` and `DELETE /api/v1/scenes/{id}` | store, overwrite or rename, and delete scenes |
| `POST /api/v1/scenes/{id}/recall` | recall a scene |
| `POST /api/v1/afv/tally`, `PUT /api/v1/afv/tally` | one source's tally change, or the full tally state |
| `POST /api/v1/loudness/reset` | restart the loudness measurement of outputs |
| `GET /api/v1/consoles`, `PUT` and `DELETE /api/v1/consoles/{id}` | the control surfaces |
| `GET /api/v1/ws` | the WebSocket: commands, deltas, meters, counters and presence |
| `GET /api/v1/session`, `POST /api/v1/login`, `GET /api/v1/login`, `POST /api/v1/logout` | sign-in |

State-changing requests without a bearer token need the header
`X-Samplez-Request: 1` and `Content-Type: application/json`:

```sh
curl -s -X PATCH http://127.0.0.1:8080/api/v1/state \
  -H 'X-Samplez-Request: 1' -H 'Content-Type: application/json' \
  -d "{\"commandId\":\"$(cat /proc/sys/kernel/random/uuid)\",\"changes\":[{\"path\":\"/channels/host/fader\",\"value\":-6}]}"
```

### Health

The ops listener serves `/healthz`, `/readyz` and `/metrics`. `/healthz`
passes while the engine's real-time thread advances its heartbeat, so it
answers 503 whenever no engine runs. `/readyz` passes once the replica is
ready and, in a set of one, the engine plays the state.

- A set of one with an `mxl` section: both pass once the engine runs.
- A set of one without an `mxl` section: both answer 503.
- A set of more than one replica, which runs no engine: `/readyz`
  follows the replica alone and passes once it has joined and caught up,
  while `/healthz` answers 503. A liveness probe on `/healthz` restarts
  such a pod over and over, so a set takes none on `/healthz`.

Besides samplez's metrics, faderz exports
`faderz_engine_resends_total`, `faderz_engine_resend_pending` and, per
channel input, `faderz_input_attached`,
`faderz_input_margin_needed_samples`, `faderz_input_copied_blocks_total`,
`faderz_input_late_blocks_total` and `faderz_input_too_late_blocks_total`.

## Control surfaces

`faderz-bridge` in `bridge/` drives Mackie Control surfaces, an X-Touch
and X-Touch Extenders, over USB-MIDI or over the network by RTP-MIDI,
and connects to faderz as a console client: fader touch and moves, motors, mute, solo, select, bank
and the scribble strips. It signs in with a token or by the OAuth 2.0
device grant. Its mapping follows the MCU reference and has not been
verified on hardware. Releases publish it as a binary and as
`ghcr.io/danielclasen/faderz-bridge:<version>`; `bridge/README.md` has
its configuration.

## Building and testing

`docker build -t faderz:local .` builds the console, the binary with the
console built into it, and the image. The Rust build needs libmxl, which
the samplez builder image carries; CI builds and tests in it:

```sh
cd web && npm ci && npm run build && cd ..
mkdir -p .cargo-registry .cargo-git
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD:/w" -v "$PWD/.cargo-registry:/usr/local/cargo/registry" \
  -v "$PWD/.cargo-git:/usr/local/cargo/git" -w /w \
  ghcr.io/danielclasen/samplez-builder@sha256:92ece4452be98fff177b49fb201c8c693130c08845c42725dd5b1173d4863fba \
  cargo test --locked
```

A build without `web/dist` serves a note in place of the console. The
console's checks, unit, contract and end-to-end tests, and its dev server
with a mock faderz, are described in `web/README.md`; `docker/smoke.sh
<image>` checks an image.

## Licence

Apache-2.0, see `LICENSE`.
