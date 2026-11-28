#!/usr/bin/env bash
set -u
HOSTIP=$(grep nameserver /etc/resolv.conf | awk '{print $2}' | head -1)
echo "hostip=$HOSTIP"
for p in 1091; do
  curl -s --max-time 20 -o /dev/null -w "socks ${HOSTIP}:${p} -> %{http_code} %{time_total}s\n" --socks5-hostname "${HOSTIP}:${p}" https://github.com || echo "socks ${p} fail"
done
# 试 mirrored localhost
curl -s --max-time 20 -o /dev/null -w "socks 127.0.0.1:1091 -> %{http_code} %{time_total}s\n" --socks5-hostname 127.0.0.1:1091 https://github.com || echo "socks local fail"
