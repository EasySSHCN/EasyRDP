#!/usr/bin/env bash
# gw-upload-051.sh —— v0.5.1 站点手工补传核对（gw-admin/admin/api/gateway）
set -e
SV=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
SITE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$SITE" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$SITE" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
DIR=$(echo "$SITE" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
echo "host=$H port=$PORT dir=$DIR"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed
chmod 600 /tmp/id_ed
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
scp -q -i /tmp/id_ed -P "${PORT:-22}" "$ROOT/easyrdp-site/gw-admin.html" "root@$H:$DIR/gw-admin.html"
scp -q -i /tmp/id_ed -P "${PORT:-22}" "$ROOT/easyrdp-site/admin.html" "root@$H:$DIR/admin.html"
ssh -q -i /tmp/id_ed -p "${PORT:-22}" "root@$H" "chown www:www $DIR/gw-admin.html $DIR/admin.html; echo issue.php: \$(grep -c register_resource $DIR/api/gateway/issue.php); echo grants-admin.php: \$(grep -c resources_list $DIR/api/gateway/grants-admin.php); echo gw-admin: \$(grep -c 资源管理 $DIR/gw-admin.html); echo admin: \$(grep -c 用户资源管理 $DIR/admin.html)"
rm -f /tmp/id_ed
echo "线上回读:"
for u in gw-admin.html admin.html; do
  echo "$u HTTP $(curl -s -o /dev/null -w '%{http_code}' https://easyrdp.cn/$u)"
done
curl -s "https://easyrdp.cn/api/gateway/issue.php" -X POST -d 'action=register_resource' -o /dev/null -w 'issue.php HTTP %{http_code}\n'
