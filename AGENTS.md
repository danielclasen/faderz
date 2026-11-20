# Contributor notes (agents and humans)

Rules for working in this repository. Read them before opening a PR or
running an automated assistant against this tree. `README.md` is the
human-facing document and wins for anything about using faderz; this
file is about changing it.

## Use a git worktree

Make every change in a worktree of its own, never in the main checkout:

```sh
git fetch origin
git worktree add .claude/worktrees/<topic>-<id> -b <topic> origin/main
```

Several sessions and editors may work on one clone at once; two writers
in one tree corrupt each other's index and branches.

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

- The state is a function of the log. A command, a snapshot and a replay
  give the same state, and the same state derives the same engine targets
  on every replica, bit for bit.
- Behaviour that a set changes is tested in a set of three replicas, not
  only in one server: equal output across replicas, failover, a replica
  that was down catching up.
- The contract schemas, the console's types (`web/src/lib/contract`) and
  the mock agree. A change to one changes all three, and the contract
  tests check it.
- Inside a `with_state` closure, take what is needed and return. Never
  call into the replica there: an entry is applied only once the closures
  running when it arrived return.

## Rolling upgrades

A set rolls pod by pod. Whether replicas of two releases can share a set
is decided by the versions and by the configuration hash.

- The versions are samplez's compute and protocol versions and faderz's
  derivation version, `DERIVATION_VERSION` in `src/state.rs`. A release
  that raises none of them rolls pod by pod with no gap in the output. A
  raised version makes the rollout a hand-over at the first new pod,
  hitless, from one committer to the next. A samplez bump can raise the
  compute or protocol version even when faderz changes nothing else.
- Raise the derivation version whenever the same log would derive a
  different state or different engine entries: a change in how a command
  applies, in target derivation, in what save and load produce, or in the
  state's saved encoding. Do not raise it for anything else. Any change
  to `apply`, target derivation, `save` or `load` is reviewed for it.
- The configuration hash, which every replica of a set must share: a
  hash that changes mid-roll leaves the set with mixed hashes and stalls
  it. It takes no build or crate version. It takes the derivation version
  as one of its inputs, at 4. Once samplez takes the derivation version
  as `Product::DERIVATION_VERSION`, faderz gives it there. The literal 4
  then stays frozen in the hash input and never moves again, like any
  other input the hash takes, and a test pins the hash for a fixed
  configuration.
- `libm` is pinned exactly, and `Cargo.lock` is committed and every build
  is `--locked`: another `libm` can compute other bits under unchanged
  versions.

A console keeps the UI it was loaded from. When the replica serving it
reports a newer build, it asks the operator to reload.

## Build and test

The server needs libmxl and a C toolchain with libclang. CI builds it in
the samplez builder image (`ghcr.io/danielclasen/samplez-builder`).

```sh
cargo fmt --all --check && (cd bridge && cargo fmt --all --check)
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo test --release --locked --test set_audio
cargo test --release --locked --test set_nmos
```

The set tests run three engines and are release-only.

The web console:

```sh
cd web && npm ci
npm run check
npm test                                  # unit and contract tests
npx playwright test <spec>                # e2e against the mock
FADERZ_E2E_BACKEND=../target/debug/faderz npx playwright test <spec>
```

The last runs the e2e against the real binary.

Before pushing a UI change, run the specs it touches against the mock and
against the binary. CI then confirms what is already known rather than
discovering it.

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
typographic dashes, quotes, arrows or ellipsis characters. State facts,
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
