#!/usr/bin/env bash
# Copy the source tree to the build/host server (default: oort-tunnel:~/condensate).
# Usage: deploy/sync.sh [ssh-host]
set -euo pipefail
HOST="${1:-oort-tunnel}"
cd "$(dirname "$0")/.."
tar --exclude=./target --exclude=./.git --exclude=node_modules --exclude=./data -czf - . \
  | ssh "$HOST" 'mkdir -p ~/condensate && tar -xzf - -C ~/condensate'
echo "synced to $HOST:~/condensate"
