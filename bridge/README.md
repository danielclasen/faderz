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
| `auth` | `{"mode": "none"}` (default) for a faderz without authentication; `{"mode": "token", "token": ...}` or `{"mode": "token", "tokenFile": ...}` for a bearer token: faderz's static `auth.token`, or an access token of the OIDC provider. |
| `units` | Left to right. `id` is the console id on the server (`^[a-z][a-z0-9-]{0,31}$`), `name` its display name, `model` `x-touch` (default) or `x-touch-extender`, `midiIn` and `midiOut` parts of the port names (`midiOut` defaults to `midiIn`). |
| `master` | The control the X-Touch master fader drives. Default: the fader of the shape's program bus. |

The surfaces run in MC mode over USB (hold CH1 SELECT at power-on to
choose it).

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
cargo test                                            # unit tests and the MIDI fixtures
FADERZ_BIN=../target/debug/faderz cargo test --features e2e --test e2e
```

The unit tests run the protocol engine and the surface mapping without
I/O. `tests/fixtures/*.mcu` replay MIDI sequences in the published MCU
message formats through the mapping and check what goes to faderz and
back to the surface. The end-to-end tests drive a faderz process with
two emulated units (`src/emulator.rs`), a software surface on an
in-process MIDI transport, and watch the state as a second client.

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
