#!/usr/bin/env bash
# enc-fix-052.sh —— 补传 EasyRDP-0.5.2-portable.exe.enc（上次模板 sed 未生效传错名）
set -e
SV=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
SITE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$SITE" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$SITE" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
DIR=$(echo "$SITE" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed
chmod 600 /tmp/id_ed
ENC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site/downloads/EasyRDP-0.5.2-portable.exe.enc
ls -l "$ENC"
scp -q -i /tmp/id_ed -P "${PORT:-22}" "$ENC" "root@$H:$DIR/downloads/EasyRDP-0.5.2-portable.exe.enc"
ssh -q -i /tmp/id_ed -p "${PORT:-22}" "root@$H" "chown www:www $DIR/downloads/EasyRDP-0.5.2-portable.exe.enc; ls -l $DIR/downloads/EasyRDP-0.5.2-portable.exe.enc"
rm -f /tmp/id_ed
curl -s -o /dev/null -w "enc HTTP %{http_code}\n" "https://easyrdp.cn/downloads/EasyRDP-0.5.2-portable.exe.enc"
