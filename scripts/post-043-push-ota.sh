#!/usr/bin/env bash
# Fix-up: GitHub push retry (v0.4.3 tag) + OTA package generation
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/EasyRDP-src-0.4.3.tar.gz"
TAG="v0.4.3"
VAULT="$ROOT/secrets-vault"
GH_TOKEN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/creds.enc" -pass file:"$VAULT/key" | sed -n 's/^github_api_token: *//p')

echo "==> GitHub push retry"
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
    echo "GitHub push OK (attempt $i)"; break
  fi
  echo "  attempt $i failed, retry..."; sleep 5
done
cd /; rm -rf "$RELDIR"

echo "==> OTA package"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$ROOT/scripts/ota-package.ps1")" -Version "0.4.3"
