#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'bash -s' <<'EOF'
mkdir -p /www/server/panel/vhost/nginx/extension/easyrdp.cn
cat > /www/server/panel/vhost/nginx/extension/easyrdp.cn/api-downloads-redirect.conf <<'RULE'
# 2026-11-19 v0.4.0: 兼容旧缓存页面/旧书签 /api/downloads/X -> /downloads/X
location ^~ /api/downloads/ {
    rewrite ^/api/downloads/(.*)$ /downloads/$1 permanent;
}
RULE
nginx -t && nginx -s reload && echo RELOADED
EOF
rm -f "$KEY"
sleep 1
curl -s -o /dev/null -w "api/downloads -> %{http_code} loc=%{redirect_url}\n" "https://easyrdp.cn/api/downloads/EasyRDP-0.4.0-portable.exe"
curl -sL -o NUL -w "follow -> %{http_code} (%{size_download}B)\n" -r 0-65535 "https://easyrdp.cn/api/downloads/EasyRDP-0.4.0-portable.exe"
