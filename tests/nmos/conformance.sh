#!/usr/bin/env bash
# Runs the AMWA nmos-testing node suites against the faderz image:
# IS-04-01 with unicast and with multicast DNS-SD, IS-05-01 and
# BCP-007-03-01, each against a freshly started faderz with an engine on
# a tmpfs MXL domain and an empty state, on one Docker bridge. Writes each
# suite's JSON results, the tool's and faderz's log to <out>, fails when a
# restart changed a resource id or the tool cloned or pulled a
# specification repository, and leaves the verdict to gate.py. The tool
# validates against the specification commits pinned in specs.txt, which
# spec-cache.sh puts into its cache.
#
# usage: tests/nmos/conformance.sh <faderz image> <tool image> <out>
#
# Absolute activations need the kernel's TAI offset set: faderz reads
# CLOCK_TAI, the tool computes TAI from UTC.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
image=$1
tool_image=$2
out=$(mkdir -p "$3" && realpath "$3")
tag=nmos-$$
net=$tag
specs=$tag-specs
# Documentation addresses: a bridge in TEST-NET-2, since the tool's DNS
# zone uses TEST-NET-1 for its unreachable registry.
subnet=198.51.100.0/24
tool_ip=198.51.100.2
node_ip=198.51.100.3
domain=testsuite.nmos.tv

offset=$(python3 -c 'import time; print(round(time.clock_gettime(time.CLOCK_TAI) - time.clock_gettime(time.CLOCK_REALTIME)))')
if [ "$offset" != 37 ]; then
  echo "CLOCK_TAI - CLOCK_REALTIME is $offset s, not 37 s" >&2
  exit 1
fi

cleanup() {
  docker rm -f "$tag-node" "$tag-tool" >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
  docker volume rm "$specs" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker network create --subnet "$subnet" "$net" >/dev/null
# Filled in the tool's image, as the user the tool runs as, so its git
# owns what it reads.
docker volume create "$specs" >/dev/null
docker run --rm -v "$specs:/cache" -v "$here:/nmos:ro" --entrypoint bash "$tool_image" \
  /nmos/spec-cache.sh /cache

# faderz's configuration: three channels, one with inputs a and b, and
# two outputs, every flow in one domain; nothing writes the input flows.
# <registry> is the nmos.registry object.
config() {
  cat <<JSON
{
  "listeners": {
    "api": { "bind": "127.0.0.1", "port": 8080 },
    "ops": { "bind": "0.0.0.0", "port": 9090 },
    "nmos": { "bind": "0.0.0.0", "port": 8081 }
  },
  "replica": { "name": "conformance" },
  "shape": {
    "sampleRate": 48000,
    "frameRate": { "numerator": 25, "denominator": 1 },
    "channels": [
      { "id": "ch1", "name": "Mic 1", "width": 1, "layout": "mono", "altInput": false },
      { "id": "ch2", "name": "Mic 2", "width": 1, "layout": "mono", "altInput": true },
      { "id": "st1", "name": "Music", "width": 2, "layout": "stereo", "altInput": false }
    ],
    "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
    "auxes": [{ "id": "aux1", "name": "Aux 1", "width": 2, "layout": "stereo" }],
    "mixMinus": [], "vcas": [], "monitorOutputs": [],
    "outputs": [
      { "id": "out-pgm", "name": "PGM out", "source": "pgm", "width": 2, "layout": "stereo" },
      { "id": "out-aux", "name": "Aux out", "source": "aux1", "width": 2, "layout": "stereo" }
    ]
  },
  "mxl": {
    "domains": [{ "name": "d", "path": "/mxl/d", "id": "6f1f1d1e-56f6-4c4c-8d32-9e7f4a3b2c10" }],
    "inputs": {
      "ch1": { "flow": "7a1e4c9b-3d2f-4b6a-8e0c-5f7d9b1a3c01" },
      "ch2": { "flow": "7a1e4c9b-3d2f-4b6a-8e0c-5f7d9b1a3c02", "altFlow": "7a1e4c9b-3d2f-4b6a-8e0c-5f7d9b1a3c03" },
      "st1": { "flow": "7a1e4c9b-3d2f-4b6a-8e0c-5f7d9b1a3c04" }
    },
    "outputs": {
      "out-pgm": { "flow": "0f3a6c2e-5b1d-4e8a-9c7f-2d4b6a8e0c01" },
      "out-aux": { "flow": "0f3a6c2e-5b1d-4e8a-9c7f-2d4b6a8e0c02" }
    }
  },
  "rt": { "enabled": false },
  "nmos": {
    "enabled": true,
    "seed": "conformance",
    "advertiseHost": "$node_ip",
    "registry": $1
  }
}
JSON
}

# Every resource id of the running node, sorted.
ids() {
  for r in self devices sources flows senders receivers; do
    curl -fsS "http://$node_ip:8081/x-nmos/node/v1.3/$r/"
    echo
  done | python3 -c 'import json,sys
ids = set()
for line in sys.stdin:
    v = json.loads(line)
    for r in (v if isinstance(v, list) else [v]):
        ids.add(r["id"])
print("\n".join(sorted(ids)))'
}

# run <name> <unicast|multicast|off> <suite> <tool suite arguments...>
run() {
  local name=$1 mode=$2 suite=$3
  shift 3
  local dns=() registry='{}'
  if [ "$mode" = unicast ]; then
    dns=(--dns "$tool_ip" --dns-search "$domain")
    registry="{ \"dnsSdDomain\": \"$domain\" }"
  fi
  config "$registry" > "$out/config-$name.json"
  docker run -d --name "$tag-node" --network "$net" --ip "$node_ip" "${dns[@]}" \
    --read-only --tmpfs /tmp --tmpfs /mxl/d:mode=1777 \
    --tmpfs /var/lib/faderz:uid=65532,gid=65532 \
    -v "$out/config-$name.json:/etc/faderz/config.json:ro" "$image" >/dev/null
  local up=
  for _ in $(seq 100); do
    curl -fsS -o /dev/null "http://$node_ip:8081/x-nmos/node/v1.3/self" && up=1 && break
    sleep 0.2
  done
  test -n "$up" || { docker logs "$tag-node" >&2; echo "$name: faderz did not serve NMOS" >&2; exit 1; }
  ids > "$out/$name.ids"
  if [ -f "$out/first.ids" ]; then
    diff "$out/first.ids" "$out/$name.ids" || { echo "resource ids changed across a restart" >&2; exit 1; }
  else
    cp "$out/$name.ids" "$out/first.ids"
  fi

  mkdir -p "$out/config-$name"
  {
    echo "from . import Config as CONFIG"
    echo "CONFIG.ENABLE_DNS_SD = $([ "$mode" = off ] && echo False || echo True)"
    echo "CONFIG.DNS_SD_MODE = '$([ "$mode" = multicast ] && echo multicast || echo unicast)'"
    echo "CONFIG.DNS_DOMAIN = '$domain'"
  } > "$out/config-$name/UserConfig.py"
  echo "== $name: $suite ($mode)"
  docker run --rm --name "$tag-tool" --network "$net" --ip "$tool_ip" \
    -v "$out/config-$name:/config:ro" -v "$out:/results" -v "$specs:/home/nmos-testing/cache" \
    --entrypoint python3 "$tool_image" nmos-test.py suite "$suite" --selection all "$@" \
    --output "/results/$name.json" 2>&1 | tee "$out/$name-tool.log" || true
  docker logs "$tag-node" > "$out/$name-node.log" 2>&1
  docker rm -f "$tag-node" >/dev/null
  test -s "$out/$name.json" || { cat "$out/$name-tool.log" >&2; echo "$name wrote no results" >&2; exit 1; }
  if grep -E "Initialising repository|Pulling latest files" "$out/$name-tool.log" >&2; then
    echo "$name: nmos-testing fetched a specification instead of the pinned cache" >&2
    exit 1
  fi
}

run is-04-01-unicast unicast IS-04-01 --host "$node_ip" --port 8081 --version v1.3
run is-04-01-multicast multicast IS-04-01 --host "$node_ip" --port 8081 --version v1.3
run is-05-01 off IS-05-01 --host "$node_ip" --port 8081 --version v1.2
run bcp-007-03-01 off BCP-007-03-01 --host "$node_ip" "$node_ip" --port 8081 8081 --version v1.3 v1.2
