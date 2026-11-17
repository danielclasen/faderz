# faderz-bridge

A faderz client for hardware control surfaces. It drives Mackie Control
(MCU) surfaces over USB-MIDI, an X-Touch and X-Touch Extenders, and
speaks the faderz WebSocket protocol (`../api/ws`) as a client of kind
`console`. Every unit is a console of its own on the server
(`/consoles`), all of them over one session.

## Running

```sh
faderz-bridge --list-ports           # the MIDI ports ALSA sees
faderz-bridge --config bridge.json
```

The release binary (`faderz-bridge-<version>-x86_64-linux`) is linked
against glibc and ALSA: a host running it needs `libasound2`.

The image (`ghcr.io/danielclasen/faderz-bridge`) runs as uid 65532 and
needs the sound devices and a writable state directory:

```sh
docker run --device /dev/snd --group-add "$(getent group audio | cut -d: -f3)" \
  -v "$PWD/bridge.json:/etc/faderz-bridge/config.json:ro" \
  -v faderz-bridge:/var/lib/faderz-bridge \
  ghcr.io/danielclasen/faderz-bridge:<version> --config /etc/faderz-bridge/config.json
```

`FADERZ_BRIDGE_LOG` sets the log level (error, warn, info, debug, trace;
info by default).

## Configuration

```json
{
  "endpoints": ["wss://faderz.example/api/v1/ws"],
  "stateDir": "/var/lib/faderz-bridge",
  "auth": { "mode": "token", "tokenFile": "/run/secrets/faderz-token" },
  "units": [
    { "id": "xtouch", "name": "X-Touch", "model": "x-touch", "midiIn": "X-Touch" },
    { "id": "xtouch-ext", "name": "X-Touch Ext", "model": "x-touch-extender", "midiIn": "X-Touch-Ext" }
  ],
  "master": "/buses/pgm/fader"
}
```

| Field | |
|---|---|
| `endpoints` | One WebSocket endpoint per faderz replica. With more than one, the bridge keeps a standby connection on another replica and fails over to it. |
| `stateDir` | Holds the session id (`session`, mode 0600), kept across restarts so a restarted bridge is the same session. Default `/var/lib/faderz-bridge`. |
| `auth` | `{"mode": "none"}` (default) for a faderz without authentication; `{"mode": "token", "token": ...}` or `{"mode": "token", "tokenFile": ...}` for a bearer token: faderz's static `auth.token`, or an access token of the OIDC provider; `{"mode": "device", ...}` to sign in at the OIDC provider (below). |
| `units` | Left to right. `id` is the console id on the server (`^[a-z][a-z0-9-]{0,31}$`), `name` its display name, `model` `x-touch` (default) or `x-touch-extender`, `midiIn` and `midiOut` parts of the port names (`midiOut` defaults to `midiIn`). |
| `master` | The control the X-Touch master fader drives. Default: the fader of the shape's program bus. |

The surfaces run in MC mode over USB (hold CH1 SELECT at power-on to
choose it).

## Signing in

In `oidc` mode faderz takes an access token of its OIDC provider as a
bearer, and the bridge gets one by the OAuth 2.0 device authorization
grant (RFC 8628):

```json
"auth": {
  "mode": "device",
  "issuer": "https://idp.example/realms/media",
  "clientId": "faderz-bridge",
  "scopes": ["offline_access"]
}
```

On start the bridge prints a verification URL and a user code, and shows
them on the first unit's scribble strip; the user opens the URL in any
browser, enters the code and signs in. The bridge polls the token
endpoint until then, and keeps the access and refresh tokens in
`<stateDir>/tokens.json`, mode 0600. A restart signs in with the stored
refresh token, without a code, while the provider still accepts it.

faderz closes a connection when its access token expires. The bridge
refreshes the token a quarter of its lifetime before that, opens a new
connection with it on the same session, makes it the primary and closes
the old one, so the session's controls and lease are kept. A refresh the
provider refuses starts a new sign-in, shown on stderr.

`issuer` is `https`, or `http` on a loopback address. `scopes` are
requested in addition to `openid`.

### What the OIDC provider needs

- A public client (no client secret) for the bridge, `faderz-bridge`
  above, with the OAuth 2.0 device authorization grant enabled. It needs
  no redirect URI and no other flow. The provider's discovery document
  has to list `device_authorization_endpoint`.
- Refresh tokens for that client. With `offline_access` among the scopes
  a provider that offers offline tokens issues one that outlives the
  user's browser session; without it the refresh token ends with that
  session, and the bridge asks for a new sign-in then.
- The access token carries what faderz checks on a bearer: `azp` naming
  the client, the claim `auth.oidc.roleClaim` with the operator role
  (`auth.oidc.operatorRole`; a viewer can watch but not write), the
  user in `preferred_username`, `email` or `sub` (the first one set),
  and, when faderz sets `auth.oidc.audience`, that audience in `aud`. The role and audience usually need mappers on the client so
  they appear in the access token and not only in the ID token.
- faderz lists the client in `auth.oidc.allowedClients`, next to its own
  client id: `"allowedClients": ["faderz", "faderz-bridge"]`.

The surface's holder then shows as the user who signed in, and the web
UI shows the bridge's controls as `<console> (<user>)`.

## Mapping

| Surface | faderz |
|---|---|
| Fader touch | `touch` of the strip's `/channels/<id>/fader`; letting go sends `release` |
| Fader move while touched | `set` in dB through the web UI's fader law (0 dB at three quarters of the travel), at most 50 per second; a move without a touch writes nothing |
| Motor | The strip's fader value whenever the fader is not touched; a held fader is never moved, and goes to the state's value when let go |
| MUTE | Toggles `/channels/<id>/mute`; the LED shows the state |
| SOLO | Toggles `/channels/<id>/solo` between `off` and `pfl`; the LED is on while it is not `off` |
| SELECT | Selects the strip's channel (`select`); the LED shows the selection, and blinks while the strip's fader is held by another session |
| REC | Read, no action |
| BANK left/right | Moves every unit by all their strips together; each unit's `bankOffset` follows |
| CHANNEL left/right | Moves by one channel |
| Master fader (X-Touch) | `master`, or the program bus's fader |
| Scribble strip | Line 1: the channel name, 7 characters. Line 2: the fader in dB, or the holder's name while another session holds it |

A touch refused because another session holds the fader shows the
holder; held for a second, it asks the holder to give the control up
(`request`). Strips past the last channel are dark.

## Connection

The bridge follows the protocol the web client follows: `hello` with
`kind: console`, `resume` from its last revision after a reconnect,
heartbeats every 250 ms, a connection dead after 750 ms of silence plus
one round-trip timeout, connect and sync deadlines sized from the
measured round trip, commands sent again after a 503 or a failover and
dropped 3 s after they were issued. After every snapshot or resume on a
new primary connection it releases every control its session owns that
no fader holds, touches again what a held fader lost, and registers its
consoles again.

## Tests

```sh
cargo test                                            # unit tests, MIDI fixtures, sign-in
FADERZ_BIN=../target/debug/faderz cargo test --features e2e --test e2e
```

The unit tests run the protocol engine and the surface mapping without
I/O. `tests/fixtures/*.mcu` replay MIDI sequences in the published MCU
message formats through the mapping and check what goes to faderz and
back to the surface. The end-to-end tests drive a faderz process with
two emulated units (`src/emulator.rs`), a software surface on an
in-process MIDI transport, and watch the state as a second client. A
stand-in OIDC provider (`tests/idp`) serves the device grant and short
access tokens, against which one of them holds a fader on an `oidc`
faderz across several token expiries.

## Not verified on hardware

No surface was available while this was built. The following follow the
reverse-engineered MCU reference and are unverified on an X-Touch or
X-Touch Extender:

- the note, pitch bend and SysEx values (touch notes 104-112, key notes,
  scribble strip SysEx `F0 00 00 66 14|15 12`), and that the Extender
  answers device id 0x15;
- that the surface needs no MCU host handshake;
- the effective fader resolution (MCU carries 14 bits, the X-Touch is
  said to resolve 10) and whether the firmware sends pitch bend without
  a touch;
- motor behaviour: that a motor moved every 20 ms at most neither hunts
  nor fights a hand;
- the LED states, the blink value 1 included;
- port names as ALSA reports them, which `midiIn` and `midiOut` match.
