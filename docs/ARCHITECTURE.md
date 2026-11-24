# faderz architecture

faderz is a mixer product on the samplez engine: samplez computes the
audio, replicates state under Raft and serves the listeners; faderz
holds the mixer's state, turns it into engine targets, and adds the API,
the web console, NMOS and the control-surface bridge. How samplez
itself is built, its real-time thread, replicated mode and the
`Product` seam, is in
[samplez's ARCHITECTURE.md](https://github.com/danielclasen/samplez/blob/main/docs/ARCHITECTURE.md).

faderz takes every samplez crate at one release tag (`tag = "vX.Y.Z"`
in `Cargo.toml`): `samplez` (the engine), `samplez-dsp`, `samplez-mxl`,
`samplez-replica`, `samplez-service` and `samplez-nmos`.

## Parts

| Path | What it is |
|---|---|
| `src/config.rs`, `src/shape.rs` | the configuration file, and the shape in the contract's `Shape` form with the state an empty start begins with |
| `src/state.rs` | the replicated mixer state, faderz's `samplez_replica::Product` |
| `src/mix.rs`, `src/strip.rs`, `src/duck.rs` | the derivation of engine targets from the state: gain matrix, strips and delays, duck groups |
| `src/replica.rs` | the state's log and snapshots on the data directory, as a set of one or several voters |
| `src/audio.rs`, `src/meters.rs` | the engine under the state, its meters as binary meter frames, its inputs' health |
| `src/app.rs`, `src/api.rs`, `src/ws.rs`, `src/contract.rs` | the REST routes, the WebSocket, what both share, and the contract compiled in |
| `src/ui.rs` | the web console, embedded from `web/dist` |
| `src/nmos.rs` | the NMOS Node and IS-05 connections |
| `src/server.rs`, `src/main.rs` | the process: replica, engine, listeners and the tasks beside them |
| `api/` | the contract: `openapi.yaml` and `ws/*.schema.json` |
| `web/` | the console in Svelte, with a mock server under `web/dev/mock` |
| `bridge/` | `faderz-bridge`, a crate of its own |

## The state and the seam to samplez

`Mixer` in `src/state.rs` is the product a samplez replica agrees on:
the live state over the configured shape (model `State` in
`api/ws/model.schema.json`), who holds which control, the last command
sequence per session and control, scenes and consoles. Every command,
from REST, the WebSocket, a tally or an NMOS activation, is appended to
the replicated log and applied by `Mixer::apply` on every replica in
log order, so every decision is made there and nowhere else: a refused
write, a scene recall's fade start, a tally's transition, a lease that
ends. `save` and `load` give the state as snapshots carry it, and
`config_hash` covers the configuration that changes how a command
applies: the shape, the declared consoles and the channels' input
wiring.

`Mixer::targets` derives samplez's absolute engine targets from the
state, and samplez diffs them into the engine entries of each log
entry. The derivation is faderz's:

- `src/mix.rs` builds the gain matrix: one row per input leg (the
  channels in shape order, then the line-up tone's generators, then the
  pre-strip rows of channels with an `input` direct out), and one
  column per output leg, then the engine meters' own columns. A
  coefficient carries the fader, mute, VCA offsets and mutes, AFV
  stage, pan or fold-down, routes, sends, bus, aux and mix-minus faders
  and mutes, polarity, mix-minus exclusions, talkback interrupts and
  the monitor's source, listens and controls.
- `src/strip.rs` turns a channel's processing into its input's strip,
  its delay and tone generator, and a bus's, aux's or mix-minus's
  processing into the strip and limiter of each output carrying it.
- `src/duck.rs` gives the duck groups' settings, keys and masks.

Fades (scene crossfades, AFV transitions) are gains with a fade the
state stamps with the sample index it starts at, so every replica's
engine ramps alike. `DERIVATION_VERSION` in `src/state.rs` versions how
a log derives state and targets; it enters samplez's compatibility key
([CONFIGURATION.md](CONFIGURATION.md#engines-in-a-set)).

## The engine

`src/audio.rs` runs the samplez engine with the configuration
`src/config.rs` builds from the `mxl` section, the matrix filled from
the live state.

- In a set of one the engine keeps its own log (`Engine::start`):
  faderz hands it the strip, duck and gain changes and input routes of
  every applied delta at the delta's `atIndex`.
- In a set of more the engine follows the replicated log
  (`Engine::start_replicated`): samplez-replica hands it the entries
  each log entry derived, and faderz only meters it and reports its
  inputs' health to the log. Each replica writes its own member flow of
  every output.

Without an `mxl` section no engine runs and the state is served alone.
The engine's meters become meter frames (`src/meters.rs`) sent to the
primary connections of clients that grant credit; its counters go out
as the `counters` message.

## The API and WebSocket server

samplez-service runs the listeners (API, ops, machine, NMOS), TLS and
the auth modes, and checks the role and CSRF header of state-changing
requests before they reach faderz. faderz passes it its replicated
state, its `/api/v1` router (`src/api.rs`), its UI router
(`src/ui.rs`), its WebSocket handler (`src/ws.rs`) and its NMOS router.

`src/app.rs` holds what REST and the WebSocket share: the replica, the
commands in flight and the fan-out of deltas, presence and selections
to every connection. The WebSocket runs over samplez-service's client
loop, which keeps heartbeats, the read-index hold and the promote
floor. Every message a client sends is checked against the contract's
JSON Schemas (`src/contract.rs`), which the console and the tests read
too.

## The web console

`web/` is a Svelte 5, Vite and TypeScript client of the contract. Its
WebSockets run in a Web Worker (`src/lib/sync/worker.ts`) around
`SyncEngine` (`src/lib/sync/engine.ts`): the primary and standby
connections, heartbeats, failover, the command outbox, meter credits
and decoding, and the replicated state; the state machines are in
`src/lib/sync/machines.ts`. The page only renders: it sends a tick per
animation frame while visible and applies the one patch the worker
answers with. `npm run build` writes `web/dist`, which the faderz build
embeds (`src/ui.rs`); a binary built without it answers `/` with 404
and a note.

`web/dev/mock` is a mock faderz implementing the same contract, for the
dev server and the console's tests ([BUILDING.md](BUILDING.md#the-console)).

## The bridge

`faderz-bridge` is a client of the WebSocket, built and released apart
from faderz. `bridge/src/engine.rs` is the protocol as a state machine
without I/O: connections, failover, commands, meter credits.
`bridge/src/surface.rs` maps the MCU units onto the state, through the
console's fader law (`bridge/src/taper.rs`); `bridge/src/mcu.rs` holds
the MCU messages, `bridge/src/midi.rs` the transports (ALSA through
midir, RTP-MIDI in `bridge/src/rtpmidi/`, and an in-process pair), and
`bridge/src/auth.rs` the device grant. `bridge/src/bridge.rs` runs them
in one task; `bridge/src/emulator.rs` is a software surface for tests.

## NMOS

`src/nmos.rs` builds on samplez-nmos: one IS-04 Node for the whole set,
a Receiver per channel (its input reader) and a Source, Flow and Sender
per output. An activation becomes a command in the replicated log, so
every replica's engine routes alike and connections survive restarts;
a scheduled activation is a command when it is staged and another the
leader orders at its time. Every replica serves the Node and Connection
APIs under the same ids, with versions taken from the replicated state,
and forwards staging to the leader, which alone registers the Node.
