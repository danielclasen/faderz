# The faderz API

The REST API, the WebSocket, the ops endpoints and NMOS. The contract is
in `api/`: `openapi.yaml` for REST under `/api/v1` and
`ws/*.schema.json` for the WebSocket at `/api/v1/ws` and the state
model. The server, the web console's types and its mock all follow it,
and the server checks what clients send against the same schemas.

## REST

| Method and path | |
|---|---|
| `GET /api/v1/state` | shape, live state, owners and the revision |
| `PATCH /api/v1/state` | write one or more controls atomically |
| `GET /api/v1/replicas` | the replica set as the serving replica sees it, and the faderz version and samplez release it was built from |
| `GET /api/v1/about` | what the serving replica was built from, and the faderz changelog of that release |
| `GET /api/v1/status` | the serving replica's role, readiness and contact state, and its engine's inputs (state, flow, blocks copied, late and too late, error) and outputs |
| `POST /api/v1/scenes`, `PUT` and `DELETE /api/v1/scenes/{id}` | store, overwrite or rename, and delete scenes |
| `POST /api/v1/scenes/{id}/recall` | recall a scene ([Scenes](USAGE.md#scenes)) |
| `POST /api/v1/afv/tally`, `PUT /api/v1/afv/tally` | one source's tally change, or the full tally state ([Audio follows video](USAGE.md#audio-follows-video)) |
| `POST /api/v1/loudness/reset` | restart the loudness measurement of outputs ([Loudness and meters](USAGE.md#loudness-and-meters)) |
| `GET /api/v1/consoles`, `PUT` and `DELETE /api/v1/consoles/{id}` | the control surfaces |
| `GET /api/v1/ws` | the WebSocket |
| `GET /api/v1/session`, `POST /api/v1/login`, `GET /api/v1/login`, `GET /api/v1/login/callback`, `POST /api/v1/logout` | sign-in |

Writes carry a client-generated command id that is applied at most once
across all replicas, and are answered once the log entry carrying them
has committed. State-changing requests without a bearer token need the
header `X-Samplez-Request: 1` and `Content-Type: application/json`:

```sh
curl -s -X PATCH http://127.0.0.1:8080/api/v1/state \
  -H 'X-Samplez-Request: 1' -H 'Content-Type: application/json' \
  -d "{\"commandId\":\"$(cat /proc/sys/kernel/random/uuid)\",\"changes\":[{\"path\":\"/channels/host/fader\",\"value\":-6}]}"
```

A viewer's writes are refused. Which requests reach the API at all
depends on the auth mode ([CONFIGURATION.md](CONFIGURATION.md#auth)).

## WebSocket

A client says `hello` with its kind (`ui` or `console`) and gets a
`snapshot` of the state, or a `resume` from the revision it had, then a
`delta` per committed change. It writes with `set`, holds and lets go
of a control with `touch` and `release`, asks a holder for a control
with `request`, shares its selection with `select`, registers surfaces
with `console`, and grants meter frames with `credit`. The server also
sends `replicas`, `counters`, `selections`, `presence`, `controlRequest`
and `ahead`, and both sides heartbeat with `ping` and `pong`. Every
connection carries the client's session id as `?session=`; a client
keeps a standby connection on another replica and `promote`s it when it
fails over. The messages are in `api/ws/client-message.schema.json` and
`api/ws/server-message.schema.json`; how the console and the bridge
use them is in [CONSOLE.md](CONSOLE.md#connection-handling) and
[SURFACES.md](SURFACES.md#connection).

## Ops endpoints

The ops listener serves `/healthz`, `/readyz` and Prometheus `/metrics`.
`/healthz` passes while the engine's real-time thread advances its
heartbeat, so it answers 503 whenever no engine runs.

| | `/healthz` | `/readyz` |
|---|---|---|
| set of one, `mxl` section | passes once the engine runs | passes once the engine plays the state |
| set of one, no `mxl` section | 503 | 503 |
| set of more, `mxl` section | passes while the engine runs, with or without a core | follows committing, as samplez's readiness describes |
| set of more, no `mxl` section | 503 | passes once the replica has joined and caught up |

A liveness probe on `/healthz` restarts a pod of a set without an `mxl`
section over and over, so such a set takes none. In a rollout, a new
pod waiting for its hand-over fails `/readyz` until it commits, so a
release that never commits stops the StatefulSet at its first new pod.
Readiness in a set is in
[samplez's REPLICAS.md](https://github.com/danielclasen/samplez/blob/main/docs/REPLICAS.md).

Besides samplez's metrics, faderz exports `faderz_afv_late_total`,
`faderz_engine_resends_total`, `faderz_engine_resend_pending` and, per
channel input, `faderz_input_attached`,
`faderz_input_margin_needed_samples`, `faderz_input_copied_blocks_total`,
`faderz_input_late_blocks_total` and `faderz_input_too_late_blocks_total`.

## NMOS

With `nmos.enabled` faderz is one NMOS Node (IS-04 v1.3, IS-05 v1.2,
AMWA BCP-007-03 for MXL): a Receiver per channel reading a flow, which
is the channel's input reader, none for a tone channel, and a Source,
Flow and Sender per output, all with transport
`urn:x-nmos:transport:mxl` and 32-bit float audio at 48 kHz. The ids
derive from `nmos.seed`, so a restart keeps them. Each domain's MXL id
comes from its `domain_def.json`, else from `mxl.domains[].id`; neither,
or two that differ, stops faderz at start.

- A receiver's `/active` flow is the flow of the channel's selected
  input: an activation sets the flow of input a or b, whichever the
  channel selects, and switching between them changes `/active`.
  `master_enable` false detaches the channel, which reads nothing and is
  silent until activated again. A flow any output of the set writes or
  merges into is refused with 400.
- A sender's flow is the output's (`mergedFlow` in a set of more), and
  `master_enable` false stops the output and removes its flow on every
  replica; `true` writes it again.
- Activations go through the replicated log like any command, so every
  replica routes alike and a restart keeps them. An activation takes
  effect once it is ordered and applied, no earlier than its activation
  time and later by at least one commit and the engine's lead; faderz
  cannot place it on a given sample. One that is not applied shows the
  state's connection again in `/active`.
- A scheduled activation is taken into the replicated state when it is
  staged, and the replica leading at its time orders it, so it fires
  after a leader change too; a cancel removes it. A scheduled activation
  taken by an earlier leader, or before a restart, shows in the
  leader's `/staged` as an absolute one at its time and locks it (423)
  until it fires or a cancel removes it; `/bulk` does not lock it. A
  scheduled activation of the connection the port already has is not
  taken.
- Every replica serves the Node and Connection APIs under the same ids
  and shows in `/active` what is in the state. The other replicas
  forward `/staged` and `/bulk` to the leader at `nmos.peers` (a set
  refuses a loopback NMOS listener without them), so staging on one
  replica and activating on another work together; while the set elects
  a leader they answer 503, and 502 or 504 when the leader cannot be
  reached or does not answer. Staged parameters not yet activated are
  lost when the leader changes.
- Only the leader registers the Node, and only while a quorum of the
  set acknowledges it; a leader cut off from the others may still post
  for up to a second while the new one registers. A replica of a set of
  more that stops leaves the Node registered for the next leader to
  continue under the same ids; a set of one deletes it.
- At start the engine reads and writes what the state holds: a detached
  channel opens no reader and a disabled sender's output creates no
  flow.
- The listener is unauthenticated (no IS-10), and its activations
  re-route inputs and start and stop outputs.

The AMWA test suites faderz passes are in
[BUILDING.md](BUILDING.md#nmos-conformance).
