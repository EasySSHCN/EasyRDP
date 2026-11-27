#!/usr/bin/env bash
# ota-upload.sh —— OTA 产物参数化上传 + 线上校验（正式版；sed 改历史模板的做法作废）
# 用法: bash scripts/ota-upload.sh <版本号，如 0.5.2>
set -e
VER="${1:?用法: bash scripts/ota-upload.sh <版本号>}"
case "$VER" in *[!0-9.]*) echo "非法版本号: $VER"; exit 1;; esac
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
SITE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$SITE" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$SITE" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
DIR=$(echo "$SITE" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
ENC_LOCAL="$ROOT/easyrdp-site/downloads/EasyRDP-$VER-portable.exe.enc"
JS_LOCAL="$ROOT/easyrdp-site/api/update/latest.json"
[ -f "$ENC_LOCAL" ] || { echo "缺 $ENC_LOCAL（先跑 scripts/ota-package.ps1 -Version $VER）"; exit 1; }
[ -f "$JS_LOCAL" ] || { echo "缺 $JS_LOCAL"; exit 1; }
grep -q "\"latestVersion\":\"$VER\"" "$JS_LOCAL" || { echo "latest.json 版本与 $VER 不一致，先重新生成"; exit 1; }
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed25519_easyssh
chmod 600 /tmp/id_ed25519_easyssh
scp -q -i /tmp/id_ed25519_easyssh -P "${PORT:-22}" \
  "$ENC_LOCAL" "root@$H:$DIR/downloads/EasyRDP-$VER-portable.exe.enc"
scp -q -i /tmp/id_ed25519_easyssh -P "${PORT:-22}" \
  "$JS_LOCAL" "root@$H:$DIR/api/update/latest.json"
ssh -q -i /tmp/id_ed25519_easyssh -p "${PORT:-22}" "root@$H" \
  "chown www:www $DIR/downloads/EasyRDP-$VER-portable.exe.enc $DIR/api/update/latest.json"
rm -f /tmp/id_ed25519_easyssh
sleep 2
CODE=$(curl -s -o /dev/null -w "%{http_code}" -r 0-1023 "https://easyrdp.cn/downloads/EasyRDP-$VER-portable.exe.enc")
JCODE=$(curl -s -o /dev/null -w "%{http_code}" "https://easyrdp.cn/api/update/latest.json?nocache=$RANDOM")
echo "enc HTTP $CODE / latest.json HTTP $JCODE"
[ "$CODE" = "206" ] && [ "$JCODE" = "200" ] && echo "✅ OTA v$VER 上传完成" || { echo "✗ 校验未过"; exit 1; }
