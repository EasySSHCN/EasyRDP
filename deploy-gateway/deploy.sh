#!/usr/bin/env bash
# EasyRDP 网关审计部署脚本 —— 在官网服务器 easyrdp.cn 上执行
# 用法：把本目录（keys.php / config.toml / easyrdp-gateway(.exe)）上传到服务器任意目录，
#       然后运行本脚本。幂等，可重复执行。
set -euo pipefail

GATEWAY_DIR=${GATEWAY_DIR:-/opt/easyrdp-gateway}   # 网关程序与 config.toml 的安装目录
SITE_API=${SITE_API:-/www/wwwroot/easyrdp.cn/api/gateway}  # 官网 api/gateway 目录
HERE=$(cd "$(dirname "$0")" && pwd)

echo "[1/4] 放置官网后台网关配置 keys.php"
if [ ! -f "$HERE/keys.php" ]; then echo "缺少 keys.php"; exit 1; fi
mkdir -p "$SITE_API"
install -m 600 "$HERE/keys.php" "$SITE_API/keys.php"
php -r '
$t = file_get_contents($argv[1]);
$j = json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/", "", $t), true);
exit(empty($j["gateway_url"]) || empty($j["gateway_admin_key"]) ? 1 : 0);
' "$SITE_API/keys.php" && echo "  keys.php 校验 OK"

echo "[2/4] 安装网关程序与配置"
mkdir -p "$GATEWAY_DIR"
BIN=$(ls "$HERE"/easyrdp-gateway* 2>/dev/null | head -n1)
[ -n "$BIN" ] || { echo "缺少网关二进制 easyrdp-gateway(.exe)"; exit 1; }
install -m 755 "$BIN" "$GATEWAY_DIR/easyrdp-gateway"
install -m 600 "$HERE/config.toml" "$GATEWAY_DIR/config.toml"

echo "[3/4] 创建 systemd 服务（Linux；Windows 服务器请用计划任务/NSSM 自行常驻）"
if command -v systemctl >/dev/null 2>&1 && [ ! -f /proc/version ] || grep -qi microsoft /proc/version 2>/dev/null; then
cat > /etc/systemd/system/easyrdp-gateway.service <<EOF
[Unit]
Description=EasyRDP Cloud Gateway
After=network.target

[Service]
ExecStart=$GATEWAY_DIR/easyrdp-gateway
WorkingDirectory=$GATEWAY_DIR
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now easyrdp-gateway
sleep 1
systemctl --no-pager -l status easyrdp-gateway | head -n 8
else
  echo "  非 systemd 环境，请手动常驻启动: $GATEWAY_DIR/easyrdp-gateway"
fi

echo "[4/4] 验证"
KEY=$(php -r '$j=json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/","",file_get_contents($argv[1])),true); echo $j["gateway_admin_key"];' "$SITE_API/keys.php")
URL=$(php -r '$j=json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/","",file_get_contents($argv[1])),true); echo $j["gateway_url"];' "$SITE_API/keys.php")
echo -n "  /admin/audit 探测: "
curl -s -H "X-Admin-Key: $KEY" "$URL/admin/audit?limit=5" | head -c 300; echo
echo "完成。回官网后台「网关审计」标签点刷新即可。"
