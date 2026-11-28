#!/usr/bin/env bash
set -uo pipefail
for JID in 130929943327 130929943384; do
  echo "=== job $JID: build/bundle 相关日志 ==="
  grep -iE "bundle|Finished|Building \[|appimage|dmg" /tmp/log-$JID.txt | grep -viE "env:|CARGO_|CACHE_ON|AZURE|VER:" | tail -25
done
