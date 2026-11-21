#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'bash -s' <<'EOF'
CONF=/www/server/panel/vhost/nginx/easyrdp.cn.conf
cp "$CONF" "$CONF.bak-api-dl-20261119"
if grep -q "api/downloads" "$CONF"; then echo "rewrite 已存在，跳过"; else
  # 在第一个 server { 行后插入兼容重写（旧缓存页面/旧客户端书签 /api/downloads/X -> /downloads/X）
  python3 - "$CONF" <<'PY'
import sys, io
p = sys.argv[1]
t = io.open(p, encoding="utf-8").read()
rule = "    # 2026-11-19 v0.4.0: 兼容旧缓存页面/旧书签的 /api/downloads/ 下载路径 → 直链\n    location ^~ /api/downloads/ {\n        rewrite ^/api/downloads/(.*)$ /downloads/$1 permanent;\n    }\n\n"
i = t.index("server {") + len("server {") + 1
t = t[:i] + "\n" + rule + t[i:]
io.open(p, "w", encoding="utf-8").write(t)
print("inserted")
PY
  nginx -t && nginx -s reload && echo "RELOADED"
fi
EOF
rm -f "$KEY"
echo "== 验证重定向 =="
curl -s -o /dev/null -w "/api/downloads -> %{http_code} loc=%{redirect_url}\n" "https://easyrdp.cn/api/downloads/EasyRDP-0.4.0-portable.exe"
