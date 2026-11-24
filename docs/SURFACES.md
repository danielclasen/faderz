# Control surfaces

`faderz-bridge` drives Mackie Control (MCU) surfaces, an X-Touch and
X-Touch Extenders, over USB-MIDI or over the network by RTP-MIDI, and
connects to faderz as a WebSocket client of kind `console`: fader touch
and moves, motors, mute, solo, select, bank, duck keys and the scribble
strips. Every unit is a console of its own in `/consoles`, all of them
over one session. Its source is in `bridge/`, a crate of its own,
released with its own version.

## Running

```sh
faderz-bridge --list-ports           # the MIDI ports ALSA sees
faderz-bridge --config bridge.json
```

Releases publish `faderz-bridge-<version>-x86_64-linux`, linked against
glibc and ALSA (a host running it needs `libasound2`), and the image
`ghcr.io/danielclasen/faderz-bridge:<version>`, which runs as uid 65532
and needs the sound devices and a writable state directory:

```sh
docker run --device /dev/snd --group-add "$(getent group audio | cut -d: -f3)" \
  -v "$PWD/bridge.json:/etc/faderz-bridge/config.json:ro" \
  -v faderz-bridge:/var/lib/faderz-bridge \
  ghcr.io/danielclasen/faderz-bridge:<version> --config /etc/faderz-bridge/config.json
```

A bridge whose units are all on RTP-MIDI needs no sound devices, only
the units' UDP ports published (`-p 5004-5005:5004-5005/udp`). Where
Docker's userland proxy forwards the ports, or rootless Docker's
default port driver does, the bridge sees the proxy's address instead
of the surface's and `allow` refuses it; `--network host` avoids that.
`bridge/deploy/kubernetes.yaml` is an example for Kubernetes: one
replica, the configuration from a ConfigMap, the state on a volume, and
the units' ports on a UDP LoadBalancer with `externalTrafficPolicy:
Local`, so the bridge sees the surfaces' own addresses, and
`loadBalancerSourceRanges`.

`FADERZ_BRIDGE_LOG` sets the log level (error, warn, info, debug, trace;
info by default). The surfaces run in MC mode (hold CH1 SELECT at
power-on to choose it).

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
| `auth` | `{"mode": "none"}` (default); `{"mode": "token", "token": ...}` or `{"mode": "token", "tokenFile": ...}` for a bearer token, faderz's `auth.token` or an access token of its OIDC provider; `{"mode": "device", ...}` to sign in at the OIDC provider ([Signing in](#signing-in)). |
| `units` | Left to right. `id` is the console id (`^[a-z][a-z0-9-]{0,31}$`), `name` its display name, `model` `x-touch` (default) or `x-touch-extender`, `transport` `midi` (default) or `rtp-midi`. With `midi`, `midiIn` and `midiOut` are parts of the port names (`midiOut` defaults to `midiIn`). |
| `master` | The control the X-Touch master fader drives. Default: the fader of the shape's program bus. |
| `keys` | The X-Touch's duck keys by MIDI note, replacing F1 to F8: `{"0": {"duck": "comm", "action": "hold"}}`. `duck` is a duck group's id; `action` is `holdOrLatch` (default), `hold` or `latch`. A note is a REC key (0 to 7), a function key (54 to 61) or another note below 104 the bridge does not map, each named once; `{}` leaves the X-Touch without duck keys. The bridge warns of keys without an X-Touch unit and of a key whose group the shape lacks. |

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
session: an invitation from another allowed address is refused while it
lasts, and one from the same address replaces it. An invitation from an
address not allowed gets no answer; the bridge logs a summary of those
at most once a minute. A peer that invited the bridge has to sync
clocks at least every two minutes or the session ends.

RTP-MIDI has no authentication and no encryption, and `allow` checks
only the UDP source address, which a sender can forge. Whoever can send
from an allowed address can open a session or take over the live one
and move faders as the bridge's user. Keep the ports off the internet,
behind a VPN or a routed network only the desks reach, and the ranges
narrow.

The bridge answers every packet at the address and port it came from,
so a peer behind a NAT works. It sends no recovery journal and skips a
received one; instead the whole surface state (motors, LEDs, scribble
strips) goes out again when a session starts, when packets from the
peer are missing, when the peer's receiver feedback stays more than
half a second behind, and every 10 seconds while the peer gives no
receiver feedback. A packet that arrives after a later one is dropped.
After packets from the peer went missing, a touched fader that does not
move within 1.5 seconds is taken as let go; its motor follows faderz
again until it is touched anew. A session that ends lets go of the
faders it held.

What goes to the surface is limited to `maxBytesPerSecond` and 512
octets per packet; a newer motor position, LED or meter level replaces
the queued one it supersedes. faderz shows a console connected while
the bridge's session holds it, not while its network surface is, so a
unit whose peer left still shows as connected.

### From a Mac

macOS shares a USB surface in a network MIDI session in Audio MIDI
Setup:

1. Connect the X-Touch by USB, in MC mode.
2. Choose Window > Show MIDI Studio, and open MIDI Network Setup
   (double-click the Network icon).
3. Under My Sessions, add a session (+) and tick it. Where macOS asks
   for the protocol, choose RTP-MIDI: the bridge does not speak Network
   MIDI 2.0.
4. Under Directory, add (+) the bridge: a name, its address as Host and
   the unit's `port` as Port. Select it and click Connect.
5. Under Live Routings, choose the X-Touch in both menus.

`allow` names the Mac's address as the bridge sees it.

### From Linux

[rtpmidid](https://github.com/davidmoreno/rtpmidid) shares an ALSA
sequencer port in an RTP-MIDI session. Add the bridge to
`/etc/rtpmidid/default.ini`, the configuration its packaged service
reads:

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
RTP-MIDI in MC mode:

1. Hold CH1 SELECT while switching the surface on: the setup menu.
2. Encoder 1: mode MC. Encoder 2: interface Network.
3. The network encoders: role MASTER, Slv IP the bridge's address,
   SlvPort the unit's `port`; DHCP, or the surface's own address, mask
   and gateway.
4. CH1 SELECT again saves.

`allow` names the surface's address, or that of the NAT in front of it.
An X-Touch and an Extender are two units, each with its own address and
port pair. In role SLAVE the surface waits on port 5004 instead, and
the unit's `invite` names it (`192.0.2.7:5004`).

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

On start the bridge prints a verification URL and a user code, and
shows them on the first unit's scribble strip; the user opens the URL
in any browser, enters the code and signs in. The bridge keeps the
access and refresh tokens in `<stateDir>/tokens.json`, mode 0600, and a
restart signs in with the stored refresh token while the provider still
accepts it. It refreshes the access token a quarter of its lifetime
before it expires, opens a new connection with it on the same session
and closes the old one, so the session's controls are kept. A refresh
the provider refuses starts a new sign-in. A refreshed token is in use
before it is written to `tokens.json`, and a failed write is retried
with backoff; tokens still unwritten when the bridge exits are lost, so
the next start uses the previous refresh token, and a provider that
rotates refresh tokens then asks for a new sign-in. `issuer` is `https`, or
`http` on a loopback address; `scopes` are requested besides `openid`.

The OIDC provider needs:

- a public client for the bridge (`faderz-bridge` above) with the
  device authorization grant enabled and no redirect URI, and
  `device_authorization_endpoint` in its discovery document. The bridge
  always sends a PKCE S256 challenge, so a provider that enforces PKCE
  works;
- refresh tokens for that client; with `offline_access` a provider that
  offers offline tokens issues one that outlives the user's browser
  session;
- an access token carrying what faderz checks on a bearer: `azp` naming
  the client, `auth.oidc.roleClaim` holding the operator role, the user
  in `preferred_username`, `email` or `sub`, and, with
  `auth.oidc.audience`, that audience in `aud`. The role and audience
  usually need mappers on the client to appear in the access token and
  not only in the ID token;
- the client in faderz's `auth.oidc.allowedClients`, next to faderz's
  own: `"allowedClients": ["faderz", "faderz-bridge"]`.

The surface's holder then shows as the user who signed in, and the
console shows the bridge's controls as `<console> (<user>)`.

## Mapping

| Surface | faderz |
|---|---|
| Fader touch | `touch` of the strip's `/channels/<id>/fader`; letting go sends `release` |
| Fader move while touched | `set` in dB through the console's fader law (0 dB at three quarters of the travel), at most 50 per second; a move without a touch writes nothing |
| Motor | The strip's fader value whenever the fader is not touched; a held fader is never moved, and goes to the state's value when let go |
| MUTE | Toggles `/channels/<id>/mute`; the LED shows the state |
| SOLO | Toggles `/channels/<id>/solo` between `off` and `pfl`; the LED is on while it is not `off` |
| SELECT | Selects the strip's channel; the LED shows the selection, and blinks while another session holds the strip's fader |
| REC | No action, unless `keys` binds it to a duck group |
| F1 to F8 (X-Touch) | The shape's duck groups in order, or what `keys` binds (below) |
| F1 to F8 LEDs | On while the group is held or its mode is `on`, blinking while it is bypassed and not held, on while it ducks its targets, else off |
| BANK left/right | Moves every unit by all their strips together |
| CHANNEL left/right | Moves by one channel |
| Master fader (X-Touch) | `master`, or the program bus's fader |
| Scribble strip | Line 1: the channel name, 7 characters. Line 2: the fader in dB, or the holder's name while another session holds it |

A touch refused because another session holds the fader shows the
holder; held for a second, it asks the holder to give the control up.
Strips past the last channel are dark.

A duck key pressed touches the group's `/ducks/<id>/hold` and sets it
`true`; let go, it releases it. Let go within 300 ms, a tap, it first
sets `/ducks/<id>/mode` to `on`, or to `auto` when it is `on`. A
bypassed group (`off`) keeps its mode, as only a screen or the API sets
and ends a bypass; its key holds only once it has been down 300 ms. A
key bound with `hold` only holds, one with `latch` only toggles the
mode. While another session holds the group, a press does not take it,
and line 2 reads `<group name> held by <holder>`; a tap still toggles
the mode. While a key of a bypassed group is down, line 2 reads
`<group name> bypassed`. After messages from a network surface went
missing, a held duck key is let go after 1.5 seconds, as a fader is. A key also lights while its group ducks, from the last meter
frame, if it is no older than 120 ms.

## Connection

The bridge speaks the protocol in
[USAGE.md](USAGE.md#websocket) with kind `console`. After every
snapshot or resume on a new primary connection it releases every
control its session owns that no fader holds, touches again what a held
fader lost, and registers its consoles again. On a shape with duck
groups the primary connection takes meter frames for the duck keys'
LEDs.

## Not verified on hardware

No surface was available while the bridge was built. The mapping
follows the published MCU message formats, and these are unverified on
an X-Touch or X-Touch Extender:

- the note, pitch bend and SysEx values (touch notes 104-112, function
  keys F1-F8 at notes 54-61, scribble strip SysEx `F0 00 00 66 14|15
  12`), and that the Extender answers device id 0x15;
- that the surface needs no MCU host handshake;
- the effective fader resolution and whether the firmware sends pitch
  bend without a touch;
- motor behaviour: that a motor moved every 20 ms at most neither hunts
  nor fights a hand;
- the LED states, the blink value 1 included;
- port names as ALSA reports them, which `midiIn` and `midiOut` match;
- RTP-MIDI with a surface's own network port, in role MASTER and SLAVE,
  across a router and a NAT, and the rate a surface takes;
- an RTP-MIDI session from macOS, and the scribble strip SysEx through
  its live routings.
