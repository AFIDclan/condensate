#!/usr/bin/env bash
# Multiplayer test: a throwaway server and three friends (alice, bob, carol), then
#   1. the protocol test (scripts/play-test.mjs): lobbies, invites, relay, kick;
#   2. with APP=/path/to/condensate-client: the same through a headless copy of the desktop app
#      (its local game API), logged in as alice. Needs xvfb-run and dbus-run-session;
#   3. with GODOT=/path/to/godot (4.3+): three headless copies of the Godot addon's self-test
#      playing together through the server.
# With DIRECT=1 the server also serves its direct address (TLS, self-signed) on another port; the
# test checks that the app and `condensate push` use it.
# Run from the repo root on Linux after `cargo build`. Needs Node 22+.
set -euo pipefail

BIN="$(pwd)/target/debug"
WORK="$(mktemp -d)"
PORT=18471
DIRECT_PORT=18472
GAME=condensate-demo
export CONDENSATE_SERVER="http://127.0.0.1:$PORT"
export CONDENSATE_PASSWORD="correct horse battery"
cleanup() {
  kill $SERVER_PID 2>/dev/null || true
  [ -n "${APP:-}" ] && pkill -f "$APP" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# With CONDENSATE_WHISPER and CONDENSATE_WHISPER_MODEL set, the server transcribes feedback
# recordings and the test checks the text.
if [ -n "${DIRECT:-}" ]; then
  export CONDENSATE_DIRECT_PORT=$DIRECT_PORT CONDENSATE_DIRECT_URL="https://127.0.0.1:$DIRECT_PORT"
fi
CONDENSATE_BIND="127.0.0.1:$PORT" CONDENSATE_DATA_DIR="$WORK/server" \
  "$BIN/condensate-server" serve >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 50); do curl -sf "$CONDENSATE_SERVER/" >/dev/null && break; sleep 0.1; done

# Each user gets their own config folder, so their own login.
cs() { XDG_CONFIG_HOME="$WORK/$1" "$BIN/condensate" "${@:2}"; }
token() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$WORK/$1/com.condensate.app/settings.json"; }
befriend() {
  for pair in "$1 $2" "$2 $1"; do
    set -- $pair
    curl -sf -X PUT -H "Authorization: Bearer $(token "$1")" "$CONDENSATE_SERVER/api/v1/friends/$2" >/dev/null
  done
}

echo "== setup"
BOOT=$(CONDENSATE_DATA_DIR="$WORK/server" "$BIN/condensate-server" invite 2>/dev/null)
cs alice register alice --display-name Alice --invite "$BOOT" >/dev/null
cs bob register bob --display-name Bob --invite "$(cs alice invite create)" >/dev/null
cs carol register carol --display-name Carol --invite "$(cs alice invite create)" >/dev/null
cs alice game create "$GAME" --title "Condensate Demo" >/dev/null
befriend alice bob
befriend alice carol

APP_FILE=""
if [ -n "${APP:-}" ]; then
  echo "== starting the desktop app as alice"
  mkdir -p "$WORK/app/com.condensate.app"
  printf '{"server":"%s","token":"%s","username":"alice"}' "$CONDENSATE_SERVER" "$(token alice)" \
    >"$WORK/app/com.condensate.app/settings.json"
  # The app's "microphone": 3 s of silence, then a sentence (synthesised by ffmpeg's flite).
  ffmpeg -loglevel error -f lavfi -i "anullsrc=r=16000:cl=mono:d=3" \
    -f lavfi -i "flite=text='The jump on the second level is way too hard. I keep falling into the water.'" \
    -filter_complex "[0][1]concat=n=2:v=0:a=1" -ar 48000 -ac 1 "$WORK/speech.wav"
  CONDENSATE_VOICE_TEST_INPUT="$WORK/speech.wav" XDG_CONFIG_HOME="$WORK/app" XDG_DATA_HOME="$WORK/appdata" \
    dbus-run-session -- xvfb-run -a "$APP" >"$WORK/app.log" 2>&1 &
  APP_FILE="$WORK/app/com.condensate.app/game-api.json"
  for _ in $(seq 100); do [ -f "$APP_FILE" ] && break; sleep 0.2; done
  [ -f "$APP_FILE" ] || { echo "FAIL: the app didn't write game-api.json"; cat "$WORK/app.log"; exit 1; }
fi

if [ -n "${DIRECT:-}" ]; then
  echo "== direct address"
  check() { if eval "$2"; then echo "  ok   $1"; else echo "  FAIL $1"; exit 1; fi; }
  INFO=$(curl -sf "$CONDENSATE_SERVER/api/v1/server")
  check "the server advertises its direct address" 'echo "$INFO" | grep -q "\"direct_url\":\"https://127.0.0.1:$DIRECT_PORT\""'
  check "the API answers over TLS on the direct port" 'curl -sfk "https://127.0.0.1:$DIRECT_PORT/api/v1/server" | grep -q server_version'
  mkdir -p "$WORK/build" && printf '#!/bin/sh\necho hi\n' >"$WORK/build/run.sh"
  PUSH=$(cs alice push --dir "$WORK/build" --game "$GAME" --platform linux-x64 --exe run.sh -m "Direct test" 2>&1)
  check "condensate push uploads over the direct address" 'echo "$PUSH" | grep -q "direct connection" && echo "$PUSH" | grep -q Published'
  if [ -n "${APP:-}" ]; then
    for _ in $(seq 50); do ss -tn state established "( dport = :$DIRECT_PORT )" | grep -q 127.0.0.1 && break; sleep 0.2; done
    check "the app connects over the direct address" 'ss -tn state established "( dport = :$DIRECT_PORT )" | grep -q 127.0.0.1'
  fi
fi

echo "== protocol"
node scripts/play-test.mjs "$CONDENSATE_SERVER" "$GAME" "$(token alice)" "$(token bob)" "$(token carol)" $APP_FILE

if [ -z "${GODOT:-}" ]; then
  echo "== Godot: skipped (set GODOT=/path/to/godot to run it)"
  exit 0
fi

echo "== Godot addon"
PROJECT="$(pwd)/sdk/godot"
"$GODOT" --headless --path "$PROJECT" --import >"$WORK/godot-import.log" 2>&1 || true
godot_as() {
  CONDENSATE_API="ws://127.0.0.1:$PORT/api/v1/play" CONDENSATE_TOKEN="$(token "$1")" CONDENSATE_GAME="$GAME" \
    timeout 60 "$GODOT" --headless --path "$PROJECT" res://demo/selftest.tscn -- "$2" >"$WORK/godot-$2.log" 2>&1
}
godot_as alice host & HOST=$!
sleep 1
godot_as bob a & A=$!
godot_as carol b & B=$!
STATUS=0
wait $HOST || STATUS=1
wait $A || STATUS=1
wait $B || STATUS=1
for r in host a b; do grep -E '^\[|PASS|FAIL|ERROR|SCRIPT ERROR' "$WORK/godot-$r.log" || true; done
if [ $STATUS -ne 0 ]; then
  echo "Godot test FAILED. Full logs:"
  tail -n 40 "$WORK"/godot-*.log
  exit 1
fi
echo "Godot test passed"
