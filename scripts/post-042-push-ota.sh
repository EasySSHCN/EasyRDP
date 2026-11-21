#!/usr/bin/env bash
# 补跑：GitHub push（v0.4.2 tag）+ OTA 加密升级包
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/EasyRDP-src-0.4.2.tar.gz"
TAG="v0.4.2"
VAULT="$ROOT/secrets-vault"
GH_TOKEN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/creds.enc" -pass file:"$VAULT/key" | sed -n 's/^github_api_token: *//p')

echo "==> GitHub push 重试"
RELDIR=$(mktemp -d)
tar -xzf "$SRC" -C "$RELDIR" --strip-components=1
cd "$RELDIR"
git init -q -b main
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "release: $TAG"
git tag -f "$TAG"
for i in 1 2 3; do
  if git -c http.version=HTTP/1.1 push -q -f \
      "https://x-access-token:${GH_TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags; then
    echo "GitHub push OK（第 $i 次尝试）"; break
  fi
  echo "  第 $i 次 push 失败，重试…"; sleep 5
done
cd /; rm -rf "$RELDIR"

echo "==> OTA 加密升级包"
ls -l "$ROOT/secrets-vault/" | grep -i ota || echo "（无 ota 密钥文件）"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$ROOT/scripts/ota-package.ps1")" -Version "0.4.2"
