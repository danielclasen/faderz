# faderz

An operated multichannel broadcast audio mixer over
[MXL](https://github.com/dmf-mxl/mxl), built on the
[samplez](https://github.com/danielclasen/samplez) engine. It reads
48 kHz audio flows, mixes them under a replicated mixer state and writes
every output to its own flow. The mixer's shape is fixed in one JSON
file; its state changes through a REST API and a WebSocket, which the
web console built into the binary and the control-surface bridge speak.

![The mixer view: channel strips, VCA masters, programme, M&E and clean-feed masters with loudness readouts, and the monitor section](docs/screenshots/console.png)

![A channel's processing panel: input, EQ curve, gate and compressor](docs/screenshots/channel-processing.png)

![The mixer on a tablet in the light theme](docs/screenshots/tablet-light.png)

## Features

- Channels of mono, stereo, 5.1 or 7.1 layout with A/B input, trim,
  polarity, high-pass, four-band EQ, gate and compressor with a parallel
  mix, a sample-exact delay and a direct out.
  [Channels](docs/USAGE.md#channels)
- Surround panning of mono channels in the ITU-R BS.775 field, and
  fold-down of wider sources onto narrower outputs.
  [Panning](docs/USAGE.md#panning), [Fold-down](docs/USAGE.md#fold-down)
- Programme, M&E and clean-feed buses, auxes with pre or post sends,
  mix-minus outputs with talkback, and VCAs; EQ, dynamics, delay and a
  true-peak limiter on bus and aux outputs.
  [Buses, auxes and mix-minus outputs](docs/USAGE.md#buses-auxes-and-mix-minus-outputs)
- A monitor section with source select, PFL and AFL, dim, cut, mono,
  fold-down and speaker mutes and solos. [Monitor](docs/USAGE.md#monitor)
- Line-up tone on chosen outputs, and tone generators as channel
  inputs. [Line-up tone](docs/USAGE.md#line-up-tone)
- Duck groups that lower ambience and music under commentary.
  [Duck groups](docs/USAGE.md#duck-groups)
- Audio follows video from a vision switcher's tally.
  [Audio follows video](docs/USAGE.md#audio-follows-video)
- Scenes with crossfades and recall-safe groups.
  [Scenes](docs/USAGE.md#scenes)
- Peak, true-peak and loudness metering against an EBU R 128, ATSC A/85
  or custom target, with phase correlation.
  [Loudness and meters](docs/USAGE.md#loudness-and-meters)
- Several operators at once: a control one session touches is held by
  it, and others see who holds it and can ask for it.
  [Sessions and control ownership](docs/USAGE.md#sessions-and-control-ownership)
- A web console for desktop, tablet and phone. [CONSOLE.md](docs/CONSOLE.md)
- Auth modes `none`, `token`, `proxy` and `oidc`, with operator and
  viewer roles. [Auth](docs/CONFIGURATION.md#auth)
- A replicated state on one replica or a set of an odd number of voters,
  with clients failing over between them.
  [A replica set](docs/CONFIGURATION.md#a-replica-set)
- NMOS IS-04 and IS-05 over MXL per AMWA BCP-007-03. [NMOS](docs/API.md#nmos)
- Mackie Control surfaces over USB-MIDI or RTP-MIDI through
  `faderz-bridge`. [SURFACES.md](docs/SURFACES.md)
- `/healthz`, `/readyz` and Prometheus `/metrics`.
  [Ops endpoints](docs/API.md#ops-endpoints)

The API is in [docs/API.md](docs/API.md), the configuration file in
[docs/CONFIGURATION.md](docs/CONFIGURATION.md), how faderz is built on
samplez in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), and building,
testing and releases in [docs/BUILDING.md](docs/BUILDING.md).

## Requirements

- Linux on x86_64, and libmxl (`/opt/mxl/lib/libmxl.so` in the image).
- MXL domains on tmpfs, writable by the process (uid 65532 in the
  image).
- For the engine's real-time thread: `CAP_SYS_NICE` and `CAP_IPC_LOCK`;
  without them faderz starts degraded and logs so.

## Quick start

Releases publish the image as `ghcr.io/danielclasen/faderz:<version>`.
This builds it from this repository and starts it with a small shape and
no `mxl` section: the state, the API and the console are served, no
engine runs and nothing is mixed.

```sh
docker build -t faderz:local .
cd "$(mktemp -d)"
mkdir -p data
cat > config.json <<'JSON'
{
  "listeners": {
    "api": { "bind": "0.0.0.0", "port": 8080 },
    "ops": { "bind": "0.0.0.0", "port": 9090 }
  },
  "replica": { "name": "studio" },
  "shape": {
    "sampleRate": 48000,
    "frameRate": { "numerator": 25, "denominator": 1 },
    "channels": [
      { "id": "host", "name": "Host", "width": 1, "layout": "mono", "altInput": false },
      { "id": "guest", "name": "Guest", "width": 1, "layout": "mono", "altInput": false },
      { "id": "band", "name": "Band", "width": 2, "layout": "stereo", "altInput": false }
    ],
    "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
    "auxes": [{ "id": "ifb", "name": "IFB", "width": 1, "layout": "mono" }],
    "mixMinus": [],
    "vcas": [{ "id": "talent", "name": "TALENT" }],
    "monitorOutputs": [{ "id": "mon", "name": "Monitor", "width": 2, "layout": "stereo" }],
    "outputs": [{ "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" }]
  }
}
JSON
docker run --rm --name faderz --user "$(id -u):$(id -g)" --read-only --tmpfs /tmp \
  -v "$PWD/config.json:/etc/faderz/config.json:ro" -v "$PWD/data:/var/lib/faderz" \
  -p 127.0.0.1:8080:8080 -p 127.0.0.1:9090:9090 \
  faderz:local
```

Open <http://127.0.0.1:8080/>, or in another shell:

```sh
curl -s http://127.0.0.1:8080/api/v1/state
curl -s http://127.0.0.1:9090/metrics
```

Without an `mxl` section `/healthz` and `/readyz` answer 503. The state is kept in `data/` and survives
a restart. With `auth.mode` `none` the API answers only requests whose
`Host` is `localhost` or `127.0.0.1` and takes WebSocket upgrades only
from those origins on the API port, so the host port above is the
container's.

### With audio

The `mxl` section names the domains and the flow each channel reads and
each output writes; every channel and every output needs one. Here the
domain is `/dev/shm/mxl/studio` on the host:

```json
"mxl": {
  "domains": [{ "name": "studio", "path": "/mxl/studio" }],
  "inputs": {
    "host": { "flow": "00000000-0000-4000-8000-000000000001" },
    "guest": { "flow": "00000000-0000-4000-8000-000000000002" },
    "band": { "flow": "00000000-0000-4000-8000-000000000003" }
  },
  "outputs": {
    "out-pgm": { "flow": "00000000-0000-4000-8000-000000000010" }
  }
}
```

```sh
docker run --rm --name faderz --read-only --tmpfs /tmp \
  --cap-drop ALL --cap-add SYS_NICE --cap-add IPC_LOCK \
  -v /dev/shm/mxl/studio:/mxl/studio \
  -v "$PWD/config.json:/etc/faderz/config.json:ro" -v faderz-data:/var/lib/faderz \
  -p 127.0.0.1:8080:8080 -p 127.0.0.1:9090:9090 \
  faderz:local
```

## Licence

Apache-2.0, see `LICENSE`.
