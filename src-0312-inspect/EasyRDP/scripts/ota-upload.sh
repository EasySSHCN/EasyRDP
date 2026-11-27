#!/usr/bin/env bash
# ota-upload.sh —— 手动补跑 release.sh 第 9/9 步的 OTA 上传 + 线上校验
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
SITE_HOST=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
echo "host=$SITE_HOST port=$SITE_PORT dir=$SITE_DIR"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed25519_easyssh
chmod 600 /tmp/id_ed25519_easyssh
ENC=$ROOT/easyrdp-site/downloads/EasyRDP-0.3.0-portable.exe.enc
JS=$ROOT/easyrdp-site/api/update/latest.json
scp -q -i /tmp/id_ed25519_easyssh -P "${SITE_PORT:-22}" "$ENC" "root@$SITE_HOST:$SITE_DIR/downloads/EasyRDP-0.3.0-portable.exe.enc"
scp -q -i /tmp/id_ed25519_easyssh -P "${SITE_PORT:-22}" "$JS" "root@$SITE_HOST:$SITE_DIR/api/update/latest.json"
echo UPLOADED
rm -f /tmp/id_ed25519_easyssh
sleep 2
CODE=$(curl -s -o /dev/null -w "%{http_code}" "https://easyrdp.cn/api/update/latest.json?nocache=$RANDOM")
echo "latest.json HTTP $CODE"
