# Building and testing faderz

How faderz, its console and the bridge are built, tested and released.
`.github/workflows/ci.yml` holds the exact commands CI runs.

## Images

```sh
docker build -t faderz:local .                                   # console, binary and image
docker build -f bridge/Dockerfile -t faderz-bridge:local .       # the bridge
```

The faderz image carries the binary with the console built into it and
the licences of everything shipped: the Rust crates (`THIRD-PARTY.txt`)
and the npm packages and fonts of the console (`THIRD-PARTY-WEB.txt`).
It runs as uid 65532. `docker/smoke.sh <image>` checks an image: its
licence notices, a start with a read-only root file system in `token`
mode, once as the calling user with every capability dropped and once
as uid 65532, that the API serves the console, refuses a request
without the token and takes one with it, that a write survives a
restart on the same data directory, and an exit 0 on SIGTERM.

## The server

The Rust build needs libmxl, which the samplez builder image carries;
CI builds and tests in it, with the cargo registry and git directories
mounted and `CARGO_INCREMENTAL=0`. Build the console first: the binary
embeds `web/dist`, and a build without it serves a note in place of the
console, which `cargo test` then checks alone.

```sh
cd web && npm ci && npm run build && cd ..
mkdir -p .cargo-registry .cargo-git
docker run --rm --user "$(id -u):$(id -g)" -e CARGO_INCREMENTAL=0 \
  -v "$PWD:/w" -v "$PWD/.cargo-registry:/usr/local/cargo/registry" \
  -v "$PWD/.cargo-git:/usr/local/cargo/git" -w /w \
  ghcr.io/danielclasen/samplez-builder@sha256:92ece4452be98fff177b49fb201c8c693130c08845c42725dd5b1173d4863fba \
  bash -euo pipefail -c '
    cargo fmt --all --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    cargo test --release --locked --test set_audio
    cargo test --release --locked --test set_nmos
  '
```

`tests/` holds the integration tests: `audio.rs` runs one server on real
MXL flows, `set*.rs` a set of three replicas, the last two in release.
`cargo deny` checks advisories, licences, bans and sources of both
crates' dependencies.

## The bridge

The bridge builds in the same image with `libasound2-dev` and
`pkg-config` installed, from `bridge/`: `cargo fmt --check`,
`cargo clippy --all-targets --all-features --locked -- -D warnings` and
`cargo test --locked`.

```sh
cargo test                                            # unit tests, MIDI fixtures, sign-in
FADERZ_BIN=../target/debug/faderz cargo test --features e2e --test e2e
FADERZ_BIN=../target/debug/faderz cargo test --features rtpmidid --test e2e rtpmidid
```

The unit tests run the protocol engine and the surface mapping without
I/O. `tests/fixtures/*.mcu` replay MIDI sequences in the published MCU
message formats through the mapping and check what goes to faderz and
back to the surface. The end-to-end tests drive a faderz process with
two emulated units (`src/emulator.rs`) on an in-process MIDI transport,
and watch the state as a second client; a stand-in OIDC provider
(`tests/idp`) serves the device grant and short access tokens, against
which one of them holds a fader on an `oidc` faderz across several token
expiries. The RTP-MIDI codec and session have unit tests with a fake
clock, and two transports of the bridge talk to each other over UDP.
With the `rtpmidid` feature the end-to-end tests also run the emulator
on a pseudo-terminal that rtpmidid (`RTPMIDID_BIN`, else on the PATH)
carries to the bridge over RTP-MIDI, in both session roles; a C compiler
and the ALSA headers build `tests/rtpmidid/noseq.c`, an empty ALSA
sequencer preloaded into rtpmidid, so the tests need no `/dev/snd`.

## The console

The console needs Node 22 and npm, from `web/`:

```sh
npm ci
npx playwright install chromium   # once, for the e2e tests
npm run dev                       # the console with a mock faderz on http://localhost:5173
npm run check                     # svelte-check; type errors and warnings fail
npm test                          # Vitest: units and contract tests
npm run test:e2e                  # Playwright against the mock
npm run build                     # production bundle in dist/
```

### The mock

The mock (`web/dev/mock/`) is loaded only under `npm run dev` and
`vite preview`; `npm run build` contains none of it. It simulates three
replicas sharing one state, binary meter frames against credit,
loudness, AFV tally cuts every 8 s and console activity (a fader ride,
bank changes, every duck group held for 4 s in every 8 s), and
validates every message it sends and receives against the contract.
Its example shape has an input that delivers nothing (VT 2, `st3`), a
5 s delay line on every channel but Music (`st1`, 100 ms), FX (`st4`)
and Line-up (`lu`, a tone generator).

- `FADERZ_MOCK_AUTOMATION=0` turns off the tally cuts and console
  activity and keys every duck group in use at a steady -50 dBFS.
- `FADERZ_MOCK_CHANNELS=<n>` sets the input channel count (default 30).
- `FADERZ_BACKEND=<url>`, such as `http://127.0.0.1:8080`, loads no
  mock: `/api/v1`, WebSocket included, goes to that faderz server.
- `FADERZ_DEV_TLS_CERT` and `FADERZ_DEV_TLS_KEY`, PEM files, serve the
  dev server over HTTPS.

The striped MOCK button at the right end of the status bar kills, hangs,
partitions and restores replicas. The same controls answer JSON under
`/__mock`, from the dev server's own origin:

| Method and path | Effect |
| --- | --- |
| `GET /__mock/replicas` | `[{id, role, healthy, frozen, partitioned, clients}]` |
| `POST /__mock/replicas/<id>/kill` | closes its sockets; new ones get 503 until restart |
| `POST /__mock/replicas/<id>/restart` | serves again: restarts a killed replica, closes the sockets of a hung one, heals a partition; `restore` is the same |
| `POST /__mock/replicas/<id>/freeze` | stays connected, sends and applies nothing; `?mode=apply` still applies |
| `POST /__mock/replicas/<id>/unfreeze` | resumes on the open sockets |
| `POST /__mock/replicas/<id>/partition` | reports `commitable: false`, answers every command with error 503 and every hello or resync with its state marked `stale`, renews no lease |
| `POST /__mock/replicas/<id>/heal` | ends the partition |
| `POST /__mock/replicas/<id>/lag?ms=<n>` | log entries reach the replica `n` ms late; `ms=0` catches up |
| `POST /__mock/blocks/hold` | keeps the current processing block open, so every WebSocket command joins one log entry until a REST request or a hello or resync ends it |
| `POST /__mock/blocks/flush` | ends the hold and commits the open block |
| `GET /__mock/stats` | per live connection: meter frames sent and skipped, credits, messages; `totals` since start or reset |
| `POST /__mock/stats/reset` | zeroes the counters |
| `GET /__mock/audit` | applied WebSocket commands in apply order, and `summary: {total, duplicates}` |
| `GET /__mock/commands` | every command outcome, REST included |
| `POST /__mock/audit/reset` | clears both command logs |
| `POST /__mock/drop-clients` | closes every socket |
| `POST /__mock/slow-link?ms=<n>` | answers every later REST request and WebSocket upgrade `n` ms late; `ms=0` ends it |
| `POST /__mock/consoles/<id>/bank?offset=<n>` | moves that console's bank to start at channel index `n` |
| `POST /__mock/ducks/script?ms=<n>` | a console session holds every duck group for `n` ms, lets go for `n` ms, and so on; `ms=0` stops |
| `POST /__mock/skip-revision` | advances the revision without a delta |
| `POST /__mock/write` | body `{"changes": [{path, value}]}`: commits the changes in one revision whether or not a replica can commit |
| `POST /__mock/rewind?by=<n>` | rolls the cluster back `n` revisions, as a restore from an older backup: a new epoch and every socket closed |

`/__mock/replicas/<id>/api/v1/ws` connects to one replica and
`/__mock/replicas/<id>/api/v1/...` serves REST through it; `/api/v1/ws`
picks a serving replica at random. The console takes a list of
same-origin endpoints as `?endpoints=/__mock/replicas/r1/api/v1/ws,/__mock/replicas/r2/api/v1/ws`.

`window.__faderz` is a read-only view of the sync state for tests
(`connection`, `standby`, `revision`, `outbox`, `failovers`, `replicas`,
`meterCredits`, `patches` and more; `session` in dev builds only).

### Console tests

The contract tests compile the schemas, resolve every `$ref` of
`openapi.yaml` and drive the mock over HTTP and WebSocket, checking
every message and response with an independent validator.

The e2e tests start three servers with `FADERZ_MOCK_AUTOMATION=0`: dev
servers on port 5181 (plain HTTP on all interfaces, for the phone
projects) and 5182 (HTTPS with a generated certificate), and a
production build under `vite preview` on 5183 with 64 channels for
`perf.spec.ts`. `FADERZ_E2E_PORT=<n>` moves them to n to n + 2. The
specs share each server's mock state and run serially; each test
restores every replica and clears the command logs and counters.

Against the faderz binary:

```sh
FADERZ_BIN=../target/debug/faderz npm run test:backend
FADERZ_E2E_BACKEND=../target/debug/faderz npm run test:e2e
FADERZ_BIN=../target/debug/faderz npm run test:binary
```

- `test:backend` starts the binary on port 6461 (`FADERZ_BACKEND_PORT`)
  and checks its REST responses against `openapi.yaml` and its
  WebSocket messages against the schemas.
- `FADERZ_E2E_BACKEND` runs the e2e suite against one replica of the
  binary behind the dev servers, with the mock's example shape less its
  delay lines and tone generator, which a binary without an engine has
  none of. Tests tagged `@mock` or `@meters` need the mock and are left
  out.
- `test:binary` starts the binary in auth modes `none`, `token`,
  `proxy` (behind a stand-in authenticating proxy,
  `tests/binary/proxy.mjs`) and `oidc` (against a stand-in OpenID
  provider, `tests/binary/idp.mjs`), from port 6471
  (`FADERZ_BINARY_PORT`), loads the console from the binary and writes
  through it within the binary's Content-Security-Policy.

Before pushing a console change, run the specs it touches against the
mock and against the binary.

### Screenshots

The screenshots in `docs/screenshots/` come from the dev mock with
invented show data (`web/tests/screenshots/vite.config.ts`):

```sh
npx playwright test -c tests/screenshots/playwright.config.ts
```

## NMOS conformance

`tests/nmos/conformance.sh` runs the AMWA nmos-testing suites IS-04-01
(unicast and multicast DNS-SD), IS-05-01 and BCP-007-03-01 against the
image, each against a freshly started faderz with an engine on a tmpfs
MXL domain. Every result is Pass but those `tests/nmos/expected.json`
lists, each with its reason, which `tests/nmos/gate.py` checks. Two of
them are Manual tests, IS-04-01 test_22 (ids kept across a restart) and
BCP-007-03-01 test_15 (`master_enable` stops and starts the flow), which
`tests/nmos.rs` covers. Absolute activations need the kernel's TAI
offset set (`tests/nmos/set-tai-offset.py`).

## CI

Every push to `main` and every pull request runs: the console's checks
and unit tests; fmt, clippy, the tests and the release set tests of the
server; the debug build checked against the contract
(`npm run test:backend`); the bridge's checks and tests and its
end-to-end tests with rtpmidid; `cargo deny`; both images, with
`docker/smoke.sh`; the e2e suite against the mock and against the
binary in three shards each, and `npm run test:binary`; and the NMOS
suites. A `ci` job passes only when every job did.

## Releases

release-please releases faderz and the bridge as two packages from one
release PR: faderz tagged `v<version>`, the bridge
`faderz-bridge-v<version>`. Merging the release PR cuts the release,
which publishes the images CI built of that commit:

- `ghcr.io/danielclasen/faderz:<version>` and
  `faderz-<version>-x86_64-linux` with its licences and SHA-256 sums;
- `ghcr.io/danielclasen/faderz-bridge:<version>` and
  `faderz-bridge-<version>-x86_64-linux` with its licences and sums.
