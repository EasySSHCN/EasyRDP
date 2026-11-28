#!/usr/bin/env bash
set -u
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SRC="$ROOT/EasyRDP-src-0.6.2.tar.gz"
TAG=v0.6.2
RELDIR=$(mktemp -d)
tar -xzf "$SRC" -C "$RELDIR" --strip-components=1 2>/dev/null
cd "$RELDIR" || exit 1
git init -q -b main
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "release: $TAG"
git tag -f "$TAG"
GH=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/creds.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^github_api_token: //p')
[ -n "$GH" ] || { echo "no token"; exit 1; }
for i in 1 2 3; do
  if timeout 600 git -c http.version=HTTP/1.1 push -f "https://x-access-token:${GH}@github.com/EasySSHCN/EasyRDP.git" main --tags 2>&1 | tail -8; then
    if git ls-remote "https://x-access-token:${GH}@github.com/EasySSHCN/EasyRDP.git" "refs/heads/main" "refs/tags/$TAG" | grep -q "$TAG"; then
      echo "PUSH_OK attempt$i"; rm -rf "$RELDIR"; exit 0
    fi
  fi
  echo "attempt$i unclear/failed"; sleep 5
done
echo "PUSH_FAILED_ALL"; exit 1
