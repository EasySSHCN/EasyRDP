#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
SERVER_ENC="$ROOT/secrets-vault/site-server.enc"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
DIR=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" -p "${PORT:-22}" "$HOST_" "mkdir -p $DIR/api/update $DIR/downloads"
scp -q -i "$KEY" -P "${PORT:-22}" "$ROOT/easyrdp-site/api/update/latest.json" "$HOST_:$DIR/api/update/latest.json" && echo "OK latest.json"
scp -q -i "$KEY" -P "${PORT:-22}" "$ROOT/easyrdp-site/downloads/EasyRDP-0.4.0-portable.exe.enc" "$HOST_:$DIR/downloads/EasyRDP-0.4.0-portable.exe.enc" && echo "OK enc"
# downloads 文件属主修复（否则 nginx 404）
ssh -i "$KEY" -p "${PORT:-22}" "$HOST_" "chown www:www $DIR/downloads/EasyRDP-0.4.0-portable.exe.enc $DIR/api/update/latest.json && ls -l $DIR/downloads/EasyRDP-0.4.0-portable.exe.enc $DIR/api/update/latest.json"
CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/api/update/latest.json")
echo "latest.json HTTP $CODE"
CODE2=$(curl -s -o /dev/null -w '%{http_code}' -r 0-1024 "https://easyrdp.cn/downloads/EasyRDP-0.4.0-portable.exe.enc")
echo "enc range HTTP $CODE2"
rm -f "$KEY"
