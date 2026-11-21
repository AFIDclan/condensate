#!/usr/bin/env bash
# End-to-end smoke test: throwaway server, register, push, pull, incremental update, repair.
# Run from the repo root on Linux after `cargo build`.
set -euo pipefail

BIN="$(pwd)/target/debug"
WORK="$(mktemp -d)"
PORT=18470
export CONDENSATE_SERVER="http://127.0.0.1:$PORT"
export XDG_CONFIG_HOME="$WORK/config"      # keep the CLI login out of the real config
export CONDENSATE_PASSWORD="correct horse battery"
trap 'kill $SERVER_PID 2>/dev/null || true; rm -rf "$WORK"' EXIT

CONDENSATE_BIND="127.0.0.1:$PORT" CONDENSATE_DATA_DIR="$WORK/server" \
  "$BIN/condensate-server" serve >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 50); do curl -sf "$CONDENSATE_SERVER/" >/dev/null && break; sleep 0.1; done

cs() { "$BIN/condensate" "$@"; }

echo "== accounts"
if cs register mallory 2>/dev/null; then echo "FAIL: registered without invite"; exit 1; fi
BOOT=$(CONDENSATE_DATA_DIR="$WORK/server" "$BIN/condensate-server" invite 2>/dev/null)
cs register alice --display-name "Alice" --invite "$BOOT"
INVITE=$(cs invite create)
cs logout >/dev/null
XDG_CONFIG_HOME="$WORK/bob" cs register bob --invite "$INVITE"
if XDG_CONFIG_HOME="$WORK/carol" cs register carol --invite "$INVITE" 2>/dev/null; then
  echo "FAIL: invite was reusable"; exit 1
fi
cs login alice

echo "== publish v1"
cs game create demo --title "Demo Game" --summary "A test"
mkdir -p "$WORK/build/data"
head -c 30000000 /dev/urandom >"$WORK/build/data/game.pck"
printf '#!/bin/sh\necho "demo game running from $(pwd)"\n' >"$WORK/build/run.sh"
cat >"$WORK/condensate.toml" <<EOF
game = "demo"
[[build]]
platform = "linux-x64"
dir = "build"
exe = "run.sh"
EOF
cs push --config "$WORK/condensate.toml" -m "First release"

echo "== bob installs"
export XDG_CONFIG_HOME_BOB="$WORK/bob"
XDG_CONFIG_HOME="$WORK/bob" cs game search demo
XDG_CONFIG_HOME="$WORK/bob" cs game add demo
XDG_CONFIG_HOME="$WORK/bob" cs pull demo "$WORK/install"
XDG_CONFIG_HOME="$WORK/bob" cs launch "$WORK/install"
cmp "$WORK/build/data/game.pck" "$WORK/install/data/game.pck"

echo "== publish v2 with a small patch inside the big file"
printf 'PATCHED' | dd of="$WORK/build/data/game.pck" bs=1 seek=15000000 conv=notrunc status=none
echo "new level" >"$WORK/build/data/level2.txt"
PUSH=$(cs --json push --config "$WORK/condensate.toml" -m "Added level 2, fixed a crash")
echo "$PUSH" | grep -E '"(version|uploaded_chunks|total_chunks|uploaded_bytes)"'

echo "== bob updates"
PULL=$(XDG_CONFIG_HOME="$WORK/bob" cs --json pull demo "$WORK/install")
echo "$PULL" | grep -E '"(from_version|to_version|downloaded_bytes|reused_bytes)"'
cmp "$WORK/build/data/game.pck" "$WORK/install/data/game.pck"
DL=$(echo "$PULL" | grep '"downloaded_bytes"' | grep -o '[0-9]*')
if [ "$DL" -gt 10000000 ]; then echo "FAIL: update downloaded $DL bytes"; exit 1; fi

echo "== up to date"
XDG_CONFIG_HOME="$WORK/bob" cs pull demo "$WORK/install"

echo "== repair after corruption"
printf 'XXXX' | dd of="$WORK/install/data/game.pck" bs=1 seek=100 conv=notrunc status=none
XDG_CONFIG_HOME="$WORK/bob" cs pull demo "$WORK/install" --repair
cmp "$WORK/build/data/game.pck" "$WORK/install/data/game.pck"

echo "== publish token (agent flow)"
TOKEN=$(cs token create --name agent --game demo 2>/dev/null)
CONDENSATE_TOKEN="$TOKEN" XDG_CONFIG_HOME="$WORK/agent" cs push --config "$WORK/condensate.toml" -m "Agent push" --version 1.0.0
if CONDENSATE_TOKEN="$TOKEN" XDG_CONFIG_HOME="$WORK/agent" cs invite create 2>/dev/null; then
  echo "FAIL: publish token could create invites"; exit 1
fi
cs versions demo

echo "== artwork"
printf '\x89PNG\r\n\x1a\nnot-really-a-png-but-sniffable' >"$WORK/cover.png"
cs game edit demo --cover "$WORK/cover.png" >/dev/null
COVER=$(cs --json game info demo | python3 -c 'import sys,json; print(json.load(sys.stdin)["cover"])')
curl -sf "$CONDENSATE_SERVER/images/$COVER" | cmp - "$WORK/cover.png"

echo "== friends, messages, playtime"
tok() { python3 -c 'import sys,json; print(json.load(open(sys.argv[1]))["token"])' "$1/com.condensate.app/settings.json"; }
ALICE=$(tok "$XDG_CONFIG_HOME"); BOB=$(tok "$WORK/bob")
api() { local who=$1 method=$2 path=$3; shift 3; curl -sf -X "$method" -H "Authorization: Bearer $who" -H 'content-type: application/json' "$CONDENSATE_SERVER/api/v1$path" "$@"; }
api "$ALICE" PUT /friends/bob >/dev/null
api "$BOB" GET /friends | grep -q '"incoming":\[{' || { echo "FAIL: bob has no incoming request"; exit 1; }
if api "$ALICE" POST /messages/bob -d '{"body":"too early"}' >/dev/null 2>&1; then echo "FAIL: messaged a non-friend"; exit 1; fi
api "$BOB" PUT /friends/alice >/dev/null
api "$ALICE" POST /messages/bob -d '{"body":"want to play demo?"}' >/dev/null
api "$BOB" GET /conversations | grep -q '"unread":1' || { echo "FAIL: bob should have 1 unread"; exit 1; }
api "$BOB" POST /messages/alice/read -d '{}' >/dev/null
api "$BOB" GET /conversations | grep -q '"unread":0' || { echo "FAIL: read receipt"; exit 1; }
api "$BOB" GET /messages/alice | grep -q 'want to play demo' || { echo "FAIL: history"; exit 1; }
api "$BOB" POST /games/demo/playtime -d '{"seconds":600}' >/dev/null
api "$BOB" GET "/library?platform=linux-x64" | grep -q '"playtime_seconds":600' || { echo "FAIL: playtime"; exit 1; }

echo "== delete version + gc"
cs game delete-version demo 0.1.0
cs versions demo | grep -q '^0.1.0' && { echo "FAIL: version not deleted"; exit 1; }
CONDENSATE_DATA_DIR="$WORK/server" "$BIN/condensate-server" gc --dry-run

echo "ALL E2E CHECKS PASSED"
