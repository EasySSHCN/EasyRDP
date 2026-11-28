#!/usr/bin/env bash
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
CFG=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$CFG" | sed -n 's/^host: \([^ ]*\).*/\1/p')
P=$(echo "$CFG" | sed -n 's/^ssh: .*port \([0-9]*\).*/\1/p')
D=$(echo "$CFG" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
printf 'H=[%s]\nP=[%s]\nD=[%s]\n' "$H" "$P" "$D"
