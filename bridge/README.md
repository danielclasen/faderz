# faderz-bridge

A faderz client for hardware control surfaces. It drives Mackie Control
(MCU) surfaces over USB-MIDI or over the network by RTP-MIDI, an X-Touch
and X-Touch Extenders, and speaks the faderz WebSocket protocol (`../api/ws`) as a client of kind
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

A bridge whose units are all on RTP-MIDI needs no sound devices, only
the units' UDP ports published (`-p 5004-5005:5004-5005/udp`).
Where Docker's userland proxy forwards the ports (Docker without its
iptables rules) or rootless Docker's default port driver does, the
bridge sees the proxy's address instead of the surface's, and `allow`
refuses it; `--network host` avoids that.
`deploy/kubernetes.yaml` is an example for Kubernetes: one replica, the
configuration from a ConfigMap, the state on a volume, and the units'
ports on a UDP LoadBalancer with `externalTrafficPolicy: Local`, so that
the bridge sees the surfaces' own addresses, and
`loadBalancerSourceRanges`.

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
| `units` | Left to right. `id` is the console id on the server (`^[a-z][a-z0-9-]{0,31}$`), `name` its display name, `model` `x-touch` (default) or `x-touch-extender`, `transport` `midi` (default) or `rtp-midi` (below). With `midi`, `midiIn` and `midiOut` are parts of the port names (`midiOut` defaults to `midiIn`). |
| `master` | The control the X-Touch master fader drives. Default: the fader of the shape's program bus. |

The surfaces run in MC mode over USB (hold CH1 SELECT at power-on to
choose it).

## Network surfaces (RTP-MIDI)

A unit with `transport` `rtp-midi` is an RTP-MIDI (AppleMIDI) session of
the bridge's own, on a control port and the data port above it:

```json
{ "id": "xtouch", "name": "X-Touch", "transport": "rtp-midi",
  "rtpMidi": { "port": 5004, "allow": ["192.0.2.0/24"] } }
```

| `rtpMidi` field | |
|---|---|
| `port` | The control port; the data port is `port + 1`. Default 5004. Every unit needs a pair of its own (an Extender next to an X-Touch: 5006). |
| `bind` | The address both ports bind to. Default `0.0.0.0`. |
| `sessionName` | The name the peer sees. Default: the unit's `name`. |
| `allow` | Addresses or ranges (`192.0.2.7`, `192.0.2.0/24`, `2001:db8::/32`) whose invitations are accepted; any other is ignored. |
| `invite` | `host:port`, the control port of a peer the bridge invites itself, resolved at start. |
| `maxBytesPerSecond` | The most the bridge sends the surface per second. Default 32000. |

A unit needs `allow`, `invite` or both. One peer at a time has a unit's
session: an invitation from another allowed address gets a refusal
while it lasts, and one from the same address replaces it (the peer
restarted). An invitation from an address not allowed gets no answer;
the bridge logs a summary of those at most once a minute.

RTP-MIDI has no authentication and no encryption, and `allow` is no
authentication either: it checks the UDP source address, which a
sender can forge. Whoever can send from an allowed address, or forge
one, can open a session or take over the live one, and move faders as
the bridge's user. Keep the ports off the internet, behind a VPN or a
routed site network that only the desks reach, and the ranges narrow.

The bridge answers every packet at the address and port it came from,
so a peer behind a NAT works. It sends no recovery journal and skips a
received one. Instead the whole surface state (motors, LEDs, scribble
strips) goes out again when a session starts, when packets from the
peer are missing, when the peer's receiver feedback stays behind what
was sent more than half a second before, and every 10 seconds while the
peer gives no receiver feedback. Such a refresh is itself sent, so with
a peer that never gives feedback it repeats every 10 seconds, also
while nothing changes. A peer's packet that
arrives after a later one is dropped, with what it carried.

What the surface sent can be lost too, a fader let go among it: after
packets from the peer went missing, a touched fader that does not move
within 1.5 seconds is taken as let go and released. A hand still on it
then writes nothing, and its motor follows faderz again, until the
fader is touched anew. A session that ends
lets go of the faders it held. A peer that invited the bridge has to
sync clocks at least every two minutes or the session ends.

What goes to the surface is limited to `maxBytesPerSecond` and 512
octets per packet. A newer motor position, LED or meter level replaces
the queued one it supersedes, and a scribble strip write drops the
queued writes it overwrites whole.

faderz shows a console connected while the bridge's session holds it,
not while its network surface is: the client protocol has no way to
report the surface gone, so a unit whose peer left still shows as
connected.

### From a Mac

macOS shares a USB surface in a network MIDI session without other
software (Audio MIDI Setup):

1. Connect the X-Touch by USB, in MC mode.
2. In Audio MIDI Setup choose Window > Show MIDI Studio, and open the
   MIDI Network Setup window (double-click the Network icon).
3. Under My Sessions, add a session (+) and tick it. Where macOS asks
   for the protocol of a new session, choose RTP-MIDI: the bridge does
   not speak Network MIDI 2.0.
4. Under Directory, add (+) the bridge: a name, its address as Host and
   the unit's `port` as Port. Select it and click Connect; it appears
   under Participants.
5. Under Live Routings, choose the X-Touch in both menus: the upper one
   passes what the X-Touch sends into the session, the lower one what
   the session receives to the X-Touch.

The bridge's `allow` names the Mac's address as the bridge sees it.

### From Linux

[rtpmidid](https://github.com/davidmoreno/rtpmidid) shares an ALSA
sequencer port in an RTP-MIDI session. Its releases have packages for
Debian, Ubuntu and Fedora:

```sh
sudo apt install ./rtpmidid-<distribution>-<arch>-<version>.deb
```

Add the bridge to `/etc/rtpmidid/default.ini`, the configuration the
packaged service reads:

```ini
[connect_to]
hostname=192.0.2.10
port=5004
name=faderz-xtouch
```

```sh
sudo systemctl restart rtpmidid
aconnect -l                # the X-Touch's port and rtpmidid's faderz-xtouch
aconnect 24:0 128:1        # X-Touch -> session (numbers from the listing)
aconnect 128:1 24:0        # session -> X-Touch
```

rtpmidid invites the bridge once something connects to its
`faderz-xtouch` port, and says bye when the last connection goes.

### An Ethernet X-Touch

The X-Touch and the X-Touch Extender have an Ethernet port that speaks
RTP-MIDI in MC mode, and can invite the bridge themselves:

1. Hold CH1 SELECT while switching the surface on: the setup menu.
2. Encoder 1: mode MC. Encoder 2: interface Network.
3. The network encoders: role MASTER, Slv IP the bridge's address,
   SlvPort the unit's `port`; DHCP, or the surface's own address, mask
   and gateway.
4. CH1 SELECT again saves.

`allow` names the surface's address, or the address of the NAT in
front of it. An X-Touch and an Extender are two units, each with its
own address and port pair. In role SLAVE the surface waits on port
5004 instead, and the unit's `invite` names it (`192.0.2.7:5004`).

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
- PKCE is optional for the provider: the bridge always sends an S256
  challenge (RFC 7636) with the device authorization request and the
  matching `code_verifier` with every token poll of that sign-in, so a
  provider that enforces PKCE on public clients works. Refresh requests
  carry no verifier.
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
FADERZ_BIN=../target/debug/faderz cargo test --features rtpmidid --test e2e rtpmidid
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

The RTP-MIDI codec and session have unit tests of their own, with a
fake clock, and two transports of the bridge talk to each other over
UDP. With the `rtpmidid` feature the end-to-end tests also run the
emulator on a pseudo-terminal that rtpmidid (`RTPMIDID_BIN`, else on
the PATH) carries to the bridge over RTP-MIDI as a raw MIDI device, in
both session roles, and restart rtpmidid. A C compiler and the ALSA
headers build `tests/rtpmidid/noseq.c`, an empty ALSA sequencer
preloaded into rtpmidid, which opens one at start; the tests need no
`/dev/snd`.

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
- port names as ALSA reports them, which `midiIn` and `midiOut` match;
- RTP-MIDI with a surface's own network port, in role MASTER and SLAVE,
  across a router and a NAT, and the rate a surface takes without
  dropping events;
- an RTP-MIDI session from macOS, and the scribble strip SysEx through
  its live routings.
