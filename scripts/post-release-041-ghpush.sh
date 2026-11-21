#!/usr/bin/env bash
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SRC="$ROOT/EasyRDP-src-0.4.1.tar.gz"
KEYF="$ROOT/secrets-vault/key"
CREDS=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/creds.enc" -pass file:"$KEYF")
GH_TOKEN=$(echo "$CREDS" | sed -n 's/^github_api_token: *//p')
RELDIR=$(mktemp -d)
tar -xzf "$SRC" -C "$RELDIR" --strip-components=1
cd "$RELDIR"
git init -q -b main
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "release: v0.4.1"
git tag -f v0.4.1
for i in 1 2 3; do
  if git -c http.version=HTTP/1.1 push -q -f \
    "https://x-access-token:${GH_TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags; then
    echo "GitHub push OK (attempt $i)"; break
  else
    echo "GitHub push attempt $i failed"; sleep 5
    [ "$i" = "3" ] && exit 1
  fi
done
rm -rf "$RELDIR"
echo done
