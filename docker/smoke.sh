#!/bin/bash
# Starts the faderz image with a read-only root file system in token
# mode, as the calling user with every capability dropped and once as the
# image's own user, 65532. Each time the API listener has to serve the
# UI, refuse a request without the token and take a write with it; the
# write has to survive a restart on the same data directory, and the
# process has to exit 0 on SIGTERM.
#
# usage: docker/smoke.sh <image>
set -euo pipefail

image=$1
name=faderz-smoke-$$
work=$(mktemp -d "${RUNNER_TEMP:-$HOME/.cache}/faderz-smoke.XXXXXX")
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  # What the image's user wrote is its own.
  docker run --rm --user 0 -v "$work:/w" --entrypoint /bin/rm "$image" -rf /w/user /w/image \
    >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

test "$(docker inspect -f '{{.Config.User}}' "$image")" = 65532:65532

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

run user --user "$(id -u):$(id -g)" --cap-drop ALL
chmod 0777 "$work/image"
run image
echo "smoke ok"
