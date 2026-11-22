#!/usr/bin/env bash
# ota-upload-0.3.3.sh 鈥斺€?鎵嬪姩琛ヨ窇 release.sh 绗?9/9 姝ワ紙v0.3.2 OTA 涓婁紶 + chown + 绾夸笂鏍￠獙锛?set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
VER=0.3.3
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
echo "host=$SITE_HOST port=$SITE_PORT dir=$SITE_DIR"
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/api/update/latest.json" "root@$SITE_HOST:$SITE_DIR/api/update/latest.json"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/downloads/EasyRDP-$VER-portable.exe.enc" "root@$SITE_HOST:$SITE_DIR/downloads/EasyRDP-$VER-portable.exe.enc"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "chown www:www $SITE_DIR/api/update/latest.json $SITE_DIR/downloads/EasyRDP-$VER-portable.exe.enc && ls -l $SITE_DIR/downloads/EasyRDP-$VER-portable.exe.enc"
rm -f "$KEY"
sleep 2
CODE=$(curl -s -o /dev/null -w "%{http_code}" "https://easyrdp.cn/api/update/latest.json?nocache=$RANDOM")
echo "latest.json HTTP $CODE"
[ "$CODE" = "200" ] || exit 1
echo "鉁?OTA v$VER 宸插彂甯?

