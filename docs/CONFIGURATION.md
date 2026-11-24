# Configuring faderz

One JSON file, `faderz --config <file>`; `faderz --help` lists the
flags. Unknown keys are refused, except under `engine` and `rt`, which
samplez checks, and only when an `mxl` section is present. How the
settings act is in [USAGE.md](USAGE.md).

## Keys

| Key | Default | |
|---|---|---|
| `listeners.api` | `127.0.0.1:8080` | `{bind, port, tls}`: `/api/v1`, `/api/v1/ws` and the console |
| `listeners.ops` | `0.0.0.0:9090` | `{bind, port, tls}`: `/healthz`, `/readyz`, `/metrics` |
| `listeners.machine` | none | `{bind, port, tls}`: a bearer-only listener for machine clients; `proxy` mode only |
| `listeners.nmos` | `127.0.0.1:8081` | `{bind, port, tls}`: the NMOS Node and Connection APIs with `nmos.enabled`, unauthenticated; bound at start, serving nothing without; `tls.mode` `none` only |
| `listeners.*.tls` | `{"mode": "none"}` | `none`; `files` with `certFile`, `keyFile` and optional `clientCaFile`, re-read when they change; `selfSigned` with `sans`, generated at start |
| `auth.mode` | `none` | `none`, `token`, `proxy` or `oidc` ([Auth](#auth)) |
| `auth.token.file` | | the API bearer token's file; required in `token` mode, optional in `oidc` mode |
| `auth.machineToken.file` | | the machine listener's bearer token; `proxy` mode |
| `auth.proxy.userHeader` | `X-Forwarded-User` | `proxy` mode: the header naming the user |
| `auth.proxy.groupsHeader` | `X-Forwarded-Groups` | `proxy` mode: comma-separated groups |
| `auth.proxy.groupRoles` | `{}` | `proxy` mode: group name to `operator` or `viewer` |
| `auth.oidc.*` | | `oidc` mode ([OpenID Connect](#openid-connect)) |
| `api.allowedHosts` | `[]` | `Host` names accepted on requests without a bearer token, besides `localhost` and `127.0.0.1` |
| `api.allowedOrigins` | `[]` | `Origin` values accepted on WebSocket upgrades, besides `localhost` and `127.0.0.1` on the API port |
| `replica.name` | the host name | the replica's id towards clients |
| `replica.dataDir` | `/var/lib/faderz` | the replicated log and snapshots |
| `replica.replicas` | `1` | the voters of the replica set, an odd count ([A replica set](#a-replica-set)) |
| `replica.ordinal` | from the name | this replica's ordinal in a set |
| `replica.peers` | | every replica's peer address, `host:port`, by ordinal |
| `replica.peer` | | `{bind, port, tls}`: the peer listener of a set |
| `replica.marker` | | the ConfigMap a set starts from; `FADERZ_REPLICA_MARKER` when the file sets none |
| `replica.maxApplyLeadMs` | `50` | 30 to 120: the cap on the lead the log stamps entries with ([Apply lead](#apply-lead)) |
| `shape` | required | the mixer's shape, model `Shape` in `api/ws/model.schema.json` |
| `consoles` | `[]` | consoles known before any registers: `id`, `name`, `protocol` (`mcu`), `transport` (`bridge`), `strips` (8) |
| `mxl.libraryPath` | `/opt/mxl/lib/libmxl.so` | libmxl, loaded at start |
| `mxl.domains` | | `[{name, path, id}]`: the mounted MXL domains; `id`, the domain's MXL id, for NMOS when the domain has no `domain_def.json` |
| `mxl.inputs.<channel>` | | `flow`; `altFlow` for a channel with `altInput`; `domain` with more than one domain; `channels`, the 1-based flow channel each leg reads; `maxDelayMs`, the channel's delay line, none by default; or `tone`, a tone generator in place of a flow, `{}` or the `frequency` and `level` it starts at |
| `mxl.outputs.<output>` | | `flow` in a set of one; `flows`, one member flow per replica by ordinal, in a set of more; `domain` with more than one domain; `mergedFlow`, the flow merging the member flows, which the NMOS sender names, in a set of more with `nmos.enabled`; `maxDelayMs`, the delay line of the bus or aux the output carries, none by default |
| `engine.sampleRate` | `48000` | |
| `engine.blockSize` | `48` | samples per block |
| `engine.readerMarginSamples` | `1536` | read delay behind libmxl's clock; delays every output by as much |
| `engine.maxCatchupBlocks` | `2` | overdue blocks processed in order after a late wake |
| `engine.pruneAfterBlocks` | `50` | blocks of a frozen producer head before an input is pruned |
| `engine.rampSamples` | `480` | the length of every gain ramp |
| `engine.delayBudgetSamples` | `4194304` | the most delay-line history the inputs may hold together, in samples; also what a replica of a set reads from another when it resyncs |
| `rt.enabled` | `true` | `false` skips capabilities, memory locking and `SCHED_FIFO` |
| `rt.priority` | `70` | `SCHED_FIFO` priority |
| `rt.rtCpu` | `auto` | `auto`, `none`, `first`, `last` or an index into the allowed CPUs |
| `rt.isolateRtCpu` | `true` | with the RT thread pinned, keeps every other thread off its CPU |
| `nmos.enabled` | `false` | register with an NMOS registry and take IS-05 connections; needs the `mxl` section |
| `nmos.seed` | replica name | what every NMOS id derives from; the StatefulSet's name in a set of more |
| `nmos.advertiseHost` | | host controllers reach `listeners.nmos` at; required with `nmos.enabled` |
| `nmos.advertisePort` | `0` | port controllers reach it at; 0 is `listeners.nmos.port` |
| `nmos.registry.url` | | Registration API, `http://host:port`; absent, DNS-SD finds one |
| `nmos.registry.dnsSdDomain` | | unicast DNS-SD domain; absent, the resolver's search domains, then mDNS |
| `nmos.heartbeatS` | `5` | 1 to 10: seconds between heartbeats |
| `nmos.asset.manufacturer` | `danielclasen` | BCP-002-02 manufacturer tag |
| `nmos.peers` | `replica.peers` hosts on `listeners.nmos.port` | `host:port` of each replica's NMOS listener by ordinal, which staging is forwarded to; in a set of more |

The `engine` and `rt` keys are samplez's; what they do is in
[samplez's USAGE.md](https://github.com/danielclasen/samplez/blob/main/docs/USAGE.md).
faderz sets the defaults of `readerMarginSamples` and
`delayBudgetSamples` above.

Without an `mxl` section faderz runs no engine: the state, the API and
the console are served, nothing is mixed, and `/healthz` and `/readyz`
fail ([API.md](API.md#ops-endpoints)). With one, every channel and every
output needs a flow (or a tone), and an MXL domain is a directory on
tmpfs shared with the processes that write and read its flows.

## Auth

| Mode | Who may write |
|---|---|
| `none` | anyone reaching the API; only requests whose `Host` is `localhost`, `127.0.0.1` or in `api.allowedHosts`, and WebSocket upgrades from the origins `http://localhost:<api port>`, `http://127.0.0.1:<api port>` or `api.allowedOrigins` |
| `token` | a client sending `auth.token` as a bearer; the console exchanges it for a session cookie |
| `proxy` | the user the proxy names in `auth.proxy.userHeader`, with the role `auth.proxy.groupRoles` gives the groups in `auth.proxy.groupsHeader` |
| `oidc` | a user faderz signs in at an OpenID Connect provider, or a bearer holding the provider's access token |

In `proxy` mode faderz takes the user the identity headers name, so it
refuses to start unless `listeners.api.bind` is a loopback address,
where only the proxy beside it reaches it; `listeners.machine` then
takes bearer-only clients. faderz logs at start which auth mode applies
and whether the file, a flag or a variable set it.

Each `auth` setting with a flag can also be given as that flag or as
its environment variable; the flag wins over the variable, the variable
over the file. `auth.mode` is `--auth-mode` and `FADERZ_AUTH_MODE`.

### OpenID Connect

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

The redirect URI to register with the provider is
`<publicUrl>/api/v1/login/callback`, and `<publicUrl>/` is the
post-logout redirect URI. `issuerUrl`, every endpoint the provider
publishes and `publicUrl` need `https`, or `http` to a loopback
address; `sessionKey` needs 32 bytes or more.

A user whose `roleClaim` holds `operatorRole` is an operator; any other
user is a viewer, or, with `viewerRole` set, a viewer only when the
claim holds that and refused otherwise. A value matches with or without
a leading `/`, as group paths are written, and a `roleClaim` with `.`
that is no top-level claim is followed as a path (`realm_access.roles`).
The role is the one at sign-in.

A session lasts `sessionTtl` from the sign-in (seconds, or a number with
`s`, `m`, `h` or `d`, 60 s to 7 d), whatever the ID token's lifetime.
Signing out ends it on the replica that answers, for every copy of the
cookie, and closes the WebSockets opened with it there; that replica
holds the sign-out in memory, so another replica, or the same one after
a restart, takes a copy of the cookie until it expires. WebSockets are
also closed when it expires. Without `sessionKey` a session holds on
the process that signed it and ends when that restarts; replicas
behind one host name share `sessionKey`.

API clients send the `auth.token` when one is configured, or an access
token of the provider: not an ID token (no `nonce`, no `typ` `ID`),
with an `azp` among `allowedClients`, and an `aud` holding `audience`
when that is set. Keycloak puts `account` into an access token's `aud`
and the client id only through an audience mapper; with `audience`
unset, `azp` alone binds the token to the allowed clients.

## A replica set

With `replica.replicas` above 1, faderz runs as one pod of a
StatefulSet per voter, reaching each other on the peer listener. How a
set starts, joins, reads ready and rolls is samplez's, in
[samplez's REPLICAS.md](https://github.com/danielclasen/samplez/blob/main/docs/REPLICAS.md);
what faderz needs on top:

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
  The marker is owned by the StatefulSet, the pod name without its
  `-<ordinal>`.
- A rollout reads the StatefulSet's target revision every 5 s, which
  needs `get` on `statefulsets`, and the pod's own revision from
  `FADERZ_POD_REVISION`, the pod's `controller-revision-hash` label
  through the downward API.
- A set starts on empty volumes. The data directory of a set of one
  cannot join a set, nor that of a set run alone, so changing between
  the two needs new volumes and leaves the state behind.
- The shape, the declared consoles and each channel's input flows and
  domain are equal on every replica (`Mixer::config_hash`): the leader
  orders no command while a replica reports another, and a replica of
  another one exits rather than install the set's snapshot. Changing
  them needs every replica stopped.
- In `oidc` mode the replicas behind one host name share
  `auth.oidc.sessionKey`.

Each replica lists every replica of the set in `GET /api/v1/replicas`
and the WebSocket's `replicas` message, named as the pods are,
`<StatefulSet>-<ordinal>` (with a `name` that ends in no ordinal, the
replica's `peers` address), with the leader raft follows, which
replicas it hears and which can commit.

### Engines in a set

With an `mxl` section every replica runs an engine of its own. Every
replica reads the same input flows and writes its own member flow of
each output: `mxl.outputs.<output>.flows` lists one flow per replica by
ordinal, and replica n writes the n-th. No two outputs or replicas name
the same flow, and none names an input's. faderz writes no flow that
merges the members; whatever merges them lists the member flows, so a
change of `flows` changes that list too. A member flow's label is the
output's name and its ordinal, `PGM out (replica 1)`; a set of one and
the NMOS sender keep the output's name.

A command that switches an input is ordered alone, and one that would
switch inputs of more than one MXL domain is refused. Without an `mxl`
section the set serves the state and mixes nothing.

The compatibility key a rollout compares hashes samplez's compute and
protocol versions, faderz's derivation version
(`DERIVATION_VERSION` in `src/state.rs`), the mixer configuration, the
`engine` settings and each channel's flow channel selection; the member
flows are left out. A release that raises none of the three versions
rolls pod by pod with no gap in the output; one that raises any rolls
as a hand-over at the first new pod, hitless. A rollback to lower
versions is no hand-over and stops the outputs for up to about 5 s.

### Apply lead

The leader stamps every log entry with a sample index a lead ahead of
the set, raised on the slack the replicas report up to
`replica.maxApplyLeadMs`; an entry a replica applies past its deadline
is not heard there until the log resolves it. A larger cap keeps a set
whose slowest follower applies late, such as one on a slowly syncing
volume, free of such entries; it is also the longest a control change
can take from the write to the audio. The mechanism is in
[samplez's ARCHITECTURE.md](https://github.com/danielclasen/samplez/blob/main/docs/ARCHITECTURE.md).
