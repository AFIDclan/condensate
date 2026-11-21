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
changed=0
for unit in condensate.service condensate-maintenance.service condensate-maintenance.timer; do
  if ! cmp -s "deploy/$unit" "/etc/systemd/system/$unit"; then
    sudo install -m 644 "deploy/$unit" "/etc/systemd/system/$unit"
    changed=1
  fi
done
if [ "$changed" = 1 ]; then
  sudo systemctl daemon-reload
  sudo systemctl enable condensate
  sudo systemctl enable --now condensate-maintenance.timer
fi
sudo systemctl restart condensate
sleep 1
systemctl --no-pager --lines=5 status condensate
curl -sf http://127.0.0.1:8470/api/v1/server && echo
EOF
