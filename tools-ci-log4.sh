#!/usr/bin/env bash
set -uo pipefail
for JID in 130929943327 130929943384; do
  echo "=== job $JID ==="
  # 找到 "No .* found" 后的 bundle 目录列表
  awk '/No \.(dmg|AppImage) found/{f=1} f&&/bundle/{p=1} f{print} f&&/exit code/{exit}' /tmp/log-$JID.txt | grep -vE "Cleaning|git config|submodule|extraheader" | head -40
done
