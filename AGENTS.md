# Contributor notes (agents and humans)

Rules for working in this repository. Read them before opening a PR or
running an automated assistant against this tree. `README.md` is the
human-facing document and wins for anything about using faderz; this
file is about changing it.

## Use a git worktree

Work in a worktree of its own, e.g.
`git worktree add .claude/worktrees/<topic> -b <topic> origin/main`, so
parallel sessions on one clone do not share an index.

## Architecture

- `src/`: the server, in Rust.
  - `state.rs`: the replicated mixer state, the `Product` faderz hands
    samplez. Every command applies here, deterministically.
  - `mix.rs` and `strip.rs`: derive engine targets (gains, strips, ducks)
    from the state.
  - `audio.rs`: drives the samplez engine with those targets, and reads
    its meters and counters.
  - `api.rs` and `ws.rs`: the REST routes and the WebSocket.
  - `app.rs`: what both share: commands in flight, fan-out, the replicas
    listing.
  - `nmos.rs`: IS-04 registration and IS-05 connections over MXL.
  - `meters.rs`: the binary meter frame.
- `api/`: the contract. `openapi.yaml` for REST and `api/ws/*.schema.json`
  for the WebSocket and the state model. The server, the web console's
  types and its mock all follow it.
- `web/`: the console in Svelte, with a mock server under `web/dev/mock`
  that implements the same contract.
- `bridge/`: the control-surface bridge, a crate of its own.
- `tests/`: integration tests. `audio.rs` runs one server on real MXL
  flows; `set*.rs` run a set of three replicas.

The audio engine, replication and NMOS stack are
[samplez](https://github.com/danielclasen/samplez), taken only at a
release tag (`tag = "vX.Y.Z"` in `Cargo.toml`). A samplez bump changes the
`Cargo.toml` tags and the samplez entries in `Cargo.lock` and nothing
else, apart from adapting to its API.

## Invariants

- The state is a function of the log: a command, a snapshot and a replay
  give the same state, and the same state derives the same engine targets
  on every replica, bit for bit. Enforced by: the state unit tests,
  `the_config_hash_covers_the_configuration_alone`, and the release-only
  set tests (`tests/set_audio.rs`, `tests/set_nmos.rs`).
- Behaviour that a set changes is tested in a set of three replicas.
  Enforced by: the set tests above, run in release in CI.
- The contract schemas, the console's types and the mock agree. Enforced
  by: `web/tests/contract` (`schemas.test.ts`, `mock.test.ts`) and
  `npm run test:backend` against the real binary.
- Inside a `with_state` closure, take what is needed and return; never
  call into the replica there. Enforced by:
  `a_replica_call_inside_with_state_is_caught` and
  `the_state_route_calls_no_replica_inside_with_state`.

## Rolling upgrades

Replicas run side by side only at equal versions: samplez's compute and
protocol versions (`samplez::COMPUTE_VERSION`,
`samplez_replica::PROTOCOL`) and faderz's derivation version
(`Product::DERIVATION_VERSION`, `DERIVATION_VERSION` in `src/state.rs`).
`rollout::key::<Mixer>` hashes them with the configuration.

- A release that raises none of them rolls pod by pod with no gap in
  the output, old and new replicas running as peers. A raised one
  changes the key, and the rollout hands over at the first new pod:
  hitless, from one committer to the next.
- Versions are ordered by compute, then protocol, then derivation. The
  baseline is handed over only to a replica whose versions are not
  below its own, so the derivation version only ever rises: never reuse
  or lower one.
- A rollback to lower versions is not a hand-over: the outputs stop
  from the last report of the last replica on the higher versions until
  the lower one takes the baseline, `rollout::DEPARTED` (3 s) plus about
  2 s. Plan a rollback as an outage of that length.
- A replica whose key differs from the baseline's sits in
  `version_mismatch` and computes nothing. A new pod waiting for its
  hand-over fails `/readyz` until it commits; the leader names it
  without reading readiness, so the hand-over does not wait on it. A
  pod below the baseline's versions passes `/readyz` while another
  audio replica commits.
- The roll brake: a release that never commits never turns a pod
  ready, so the StatefulSet stops at its first new pod.
- A samplez bump can raise compute or protocol on its own; its
  changelog says so.
- Raise the derivation version for any change in how a command
  applies, in target derivation, in save or load, or in the state's
  saved encoding, and for nothing else. Any change to `apply`, target
  derivation, `save` or `load` is reviewed for it.
- The configuration hash (`Mixer::config_hash`) takes no build or crate
  version. Its inputs, the literal `4` included, stay frozen, so a
  release keeps it; `the_config_hash_of_a_fixed_configuration_never_moves`
  pins it.
- `libm` stays pinned at `=0.2.16`, `Cargo.lock` is committed and every
  build is `--locked`: another `libm` can compute other bits.

The release that takes samplez 0.17.0 changes the key's formula itself,
so its own roll is a hand-over. It also moves the loudness reset and
pause into the derived targets, which would raise the derivation
version; it stays at 4 there only because that release is the first
under explicit versions, whose roll is a hand-over anyway.

The release that takes samplez v0.20.0 runs at
`samplez::COMPUTE_VERSION` 2 (from 1), `samplez_replica::PROTOCOL` 4
(from 3) and `DERIVATION_VERSION` 10 (from 9), the state recording each
IS-05 port's version for the shared NMOS versions. Its roll is a
hand-over.

The release that takes samplez v0.22.0 runs at
`samplez::COMPUTE_VERSION` 2 and `samplez_replica::PROTOCOL` 5 (from
4), with `DERIVATION_VERSION` still 10. PROTOCOL 5 makes its roll a
hand-over.

The release that applies bus, aux and mix-minus EQ and dynamics and
bus and aux delay on their outputs runs at `DERIVATION_VERSION` 11
(from 10).

## Build and test

`.github/workflows/ci.yml` holds the exact commands; run them as CI does.
In short:

- The server builds in the samplez builder image
  (`ghcr.io/danielclasen/samplez-builder`), with the cargo registry and
  git directories mounted and `CARGO_INCREMENTAL=0`. Build the console
  first (`cd web && npm ci && npm run build`): the binary embeds
  `web/dist`. Then `cargo fmt --all --check`,
  `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked`, and in release the set tests.
- The bridge (`bridge/`) has its own fmt, clippy and test, and needs
  `libasound2-dev`.
- The console: `npm run check`, `npm test`, and `npx playwright test`
  against the mock or, with `FADERZ_E2E_BACKEND`, against the binary.
  `npm run test:backend` and `npm run test:binary` run in CI against the
  built binary.

Before pushing a UI change, run the specs it touches against the mock and
against the binary.

## Tests

- A test proves the change: it fails without it and passes with it.
  Check that by breaking the code once, before relying on the test.
- Timing is judged in ways a loaded runner cannot break: poll for a
  condition with a deadline, count work, or judge a statistic. Never
  read a fixed number of frames after a fixed wait.
- Audio assertions are independent of the sample and frame rate.
- No skipped tests and no retries that hide a flake. Fix a flake at its
  cause.

## User interfaces

A control that starts an action which can take longer than 100 ms shows
that it is working from the moment it is pressed until the action
completes or fails. That means a busy state on the control itself,
disabled against repeat presses, and a visible error if it fails.

## Working style

- Read the code you change, and its callers and tests, before writing.
- State assumptions, and surface tradeoffs rather than picking silently.
- The minimum code that solves the problem: no speculative options,
  abstractions or error handling for impossible states.
- Fail loudly. Work is not complete while anything was skipped, and tests
  do not pass while any were skipped.

## Text

ASCII only in code, comments, commit messages and PR descriptions: no
typographic dashes, quotes, arrows or ellipsis characters. The vendored
skills under `.agents/skills/` are exempt. State facts,
and leave out what cannot be verified from the source, a run or the
authoritative documentation.

## Commits and pull requests

- Conventional Commits with a scope (`fix(audio): ...`), with `!` or a
  `BREAKING CHANGE:` footer for breaking changes. The subject is at most
  72 characters, in the imperative mood; the body wraps at 72.
- A message says why the change exists, in terms still useful read alone
  years later. Write it declaratively, with no narration of what was
  tried, no checklists and no tool-authored `Co-Authored-By` trailers.
- PRs are squash-merged with the prefilled body. release-please takes one
  changelog entry per squash, from the PR title. A PR carrying several
  release-relevant changes uses a `BEGIN_COMMIT_OVERRIDE` block in its
  body.
- Merging is not releasing: merging the release PR release-please opens
  cuts the release.
- Every PR gets an independent review.

## Graphify

A [Graphify](https://github.com/safishamsi/graphify) knowledge graph can
be built locally with `graphify update .`. It is not committed; the
skill and hook are, under `.agents/skills/`. For codebase questions,
prefer `graphify query "<question>"` over wide searches once
`graphify-out/graph.json` exists.

## When in doubt

Ask the maintainer before changing the public API or the contract, the
release strategy, or anything hard to reverse.
