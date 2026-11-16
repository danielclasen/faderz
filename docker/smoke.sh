#!/bin/bash
# Checks the image carries its licence notices, then starts it with a
# read-only root file system in token mode, as the calling user with
# every capability dropped and once as the image's own user, 65532.
# With an audio engine configured, the image's own user has to get its
# RT thread and locked memory from SYS_NICE and IPC_LOCK added to the
# container, and has to start degraded without them.
# Each time the API listener has to serve the UI, refuse a request
# without the token and take a write with it; the write has to survive
# a restart on the same data directory, and the process has to exit 0
# on SIGTERM.
#
# usage: docker/smoke.sh <image>
set -euo pipefail

image=$1
name=faderz-smoke-$$
work=$(mktemp -d "${RUNNER_TEMP:-$HOME/.cache}/faderz-smoke.XXXXXX")
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1
  # What the image's user wrote is its own.
  docker run --rm --user 0 -v "$work:/w" --entrypoint /bin/rm "$image" -rf /w/user /w/image \
    >/dev/null 2>&1
  rm -rf "$work"
}
trap cleanup EXIT

test "$(docker inspect -f '{{.Config.User}}' "$image")" = 65532:65532
for notice in LICENSE THIRD-PARTY.txt THIRD-PARTY-WEB.txt; do
  docker run --rm --entrypoint /bin/test "$image" -s "/usr/share/licenses/faderz/$notice"
done

token=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
mkdir "$work/user" "$work/image" "$work/secret"
printf '%s\n' "$token" > "$work/secret/token"
cat > "$work/config.json" <<JSON
{
  "listeners": {
    "api": { "bind": "0.0.0.0", "port": 8080 },
    "ops": { "bind": "0.0.0.0", "port": 9090 }
  },
  "auth": { "mode": "token", "token": { "file": "/run/secrets/faderz/token" } },
  "replica": { "name": "smoke" },
  "shape": {
    "sampleRate": 48000,
    "frameRate": { "numerator": 25, "denominator": 1 },
    "channels": [{ "id": "ch1", "name": "Mic 1", "width": 1, "layout": "mono", "altInput": false }],
    "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
    "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [], "outputs": []
  }
}
JSON

api=
data=
start() {
  docker run -d --name "$name" --read-only --tmpfs /tmp \
    -v "$work/$data:/var/lib/faderz" -v "$work/secret:/run/secrets/faderz:ro" \
    -v "$work/config.json:/etc/faderz/config.json:ro" \
    -p 127.0.0.1::8080 "$@" "$image" >/dev/null
  api=$(docker port "$name" 8080/tcp | head -n1)
  for _ in $(seq 50); do
    curl -sS -o /dev/null "http://$api/" 2>/dev/null && return
    sleep 0.2
  done
  docker logs "$name"
  exit 1
}

stop() {
  docker stop -t 10 "$name" >/dev/null
  test "$(docker inspect -f '{{.State.ExitCode}}' "$name")" = 0
  docker rm "$name" >/dev/null
}

# run <data directory under $work> <docker run options...>
run() {
  data=$1
  shift
  echo "== faderz with $*"
  start "$@"
  curl -fsS "http://$api/" | grep -q '<script'
  test "$(curl -s -o /dev/null -w '%{http_code}' "http://$api/api/v1/state")" = 401
  local bearer=(-H "Authorization: Bearer $token")
  local id
  id=$(cat /proc/sys/kernel/random/uuid)
  curl -fsS "${bearer[@]}" -H 'Content-Type: application/json' -X PATCH \
    -d "{\"commandId\":\"$id\",\"changes\":[{\"path\":\"/channels/ch1/fader\",\"value\":-7}]}" \
    "http://$api/api/v1/state" | grep -q '"result":"applied"'
  stop
  start "$@"
  curl -fsS "${bearer[@]}" "http://$api/api/v1/state" | grep -q '"fader":-7'
  stop
}

# rt <expected sched and mlock state> <docker run options...>
rt() {
  local state=$1 detail=
  shift
  test "$state" = Active && detail=SCHED_FIFO
  echo "== faderz engine with $*"
  cat > "$work/rt.json" <<JSON
{
  "listeners": { "api": { "bind": "0.0.0.0", "port": 8080 }, "ops": { "bind": "0.0.0.0", "port": 9090 } },
  "auth": { "mode": "token", "token": { "file": "/run/secrets/faderz/token" } },
  "replica": { "name": "smoke" },
  "shape": {
    "sampleRate": 48000,
    "frameRate": { "numerator": 25, "denominator": 1 },
    "channels": [{ "id": "ch1", "name": "Mic 1", "width": 1, "layout": "mono", "altInput": false }],
    "buses": [{ "id": "pgm", "name": "PGM", "kind": "program", "width": 2, "layout": "stereo" }],
    "auxes": [], "mixMinus": [], "vcas": [], "monitorOutputs": [],
    "outputs": [{ "id": "out", "name": "Out", "source": "pgm", "width": 2, "layout": "stereo" }]
  },
  "mxl": {
    "domains": [{ "name": "d", "path": "/mxl/d" }],
    "inputs": { "ch1": { "flow": "7a1e4c9b-3d2f-4b6a-8e0c-5f7d9b1a3c55" } },
    "outputs": { "out": { "flow": "0f3a6c2e-5b1d-4e8a-9c7f-2d4b6a8e0c11" } }
  }
}
JSON
  docker run -d --name "$name" --read-only --user 65532:65532 \
    --tmpfs /tmp --tmpfs /mxl/d:mode=1777 --tmpfs /var/lib/faderz:uid=65532,gid=65532 \
    -v "$work/secret:/run/secrets/faderz:ro" -v "$work/rt.json:/etc/faderz/config.json:ro" \
    "$@" "$image" >/dev/null
  local line=
  for _ in $(seq 50); do
    line=$(docker logs "$name" 2>&1 | grep '^faderz: rt ' || true)
    test -n "$line" && break
    sleep 0.2
  done
  test -n "$line" || { docker logs "$name"; exit 1; }
  grep -q "sched: $state(\"$detail" <<<"$line" \
    && grep -q "mlock: $state(\"${detail:+MCL_CURRENT}" <<<"$line" \
    || { echo "sched and mlock are not $state: $line"; exit 1; }
  docker rm -f "$name" >/dev/null
}

run user --user "$(id -u):$(id -g)" --cap-drop ALL
chmod 0777 "$work/image"
run image
rt Active --cap-drop ALL --cap-add SYS_NICE --cap-add IPC_LOCK
rt Degraded --cap-drop ALL
echo "smoke ok"
