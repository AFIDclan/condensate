#!/usr/bin/env bash
# Build a release on the server and (re)start the systemd service.
# Run from your dev machine: deploy/deploy-server.sh [ssh-host]
set -euo pipefail
HOST="${1:-oort-tunnel}"
cd "$(dirname "$0")/.."
bash deploy/sync.sh "$HOST"
ssh "$HOST" bash -s <<'EOF'
set -euo pipefail
cd ~/condensate
~/.cargo/bin/cargo build --release -p condensate-server -p condensate-cli
mkdir -p ~/condensate-server/bin ~/condensate-server/data
install -m 755 target/release/condensate-server target/release/condensate ~/condensate-server/bin/
if ! cmp -s deploy/condensate.service /etc/systemd/system/condensate.service; then
  sudo install -m 644 deploy/condensate.service /etc/systemd/system/condensate.service
  sudo systemctl daemon-reload
  sudo systemctl enable condensate
fi
sudo systemctl restart condensate
sleep 1
systemctl --no-pager --lines=5 status condensate
curl -sf http://127.0.0.1:8470/api/v1/server && echo
EOF
