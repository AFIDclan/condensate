#!/bin/sh
# Accept the Condensate direct port ahead of Tailscale's firewall rules.
#
# Tailscale's `ts-input` chain drops anything from 100.64.0.0/10 that doesn't arrive over
# Tailscale. Some ISPs use that range for carrier-grade NAT, so friends on those ISPs (and
# anyone on the same ISP as the server) would time out on the port forward without this.
# Installed as /usr/local/sbin/condensate-firewall and run by condensate-firewall.service.
set -eu
PORT="${1:-7755}"
while iptables -D INPUT -p tcp --dport "$PORT" -m comment --comment condensate-direct -j ACCEPT 2>/dev/null; do :; done
iptables -I INPUT 1 -p tcp --dport "$PORT" -m comment --comment condensate-direct -j ACCEPT
