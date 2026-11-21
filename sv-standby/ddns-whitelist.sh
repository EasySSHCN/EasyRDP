#!/bin/bash
# =============================================================================
# ddns-whitelist.sh — 家宽动态 IP 防火墙白名单守护（DDNS 拉取版）
# 作用：解析 home.7xm.net，IP 变化时自动更新本机 firewalld rich rule 白名单。
# 覆盖端口由 PORTS 变量定义（SSH 端口 + 宝塔面板端口），sv 备机拉取 IP 为固定
# 白名单（104.233.200.122），不在本脚本管理范围。
# 由 root crontab 每 5 分钟调用；状态文件 /var/lib/ddns-whitelist.current。
# =============================================================================
DDNS_HOST="home.7xm.net"
STATE="/var/lib/ddns-whitelist.current"
PORTS="__PORTS__"

NEW="$(dig +short "$DDNS_HOST" A 2>/dev/null | grep -E '^[0-9]{1,3}(\.[0-9]{1,3}){3}$' | tail -1)"
[ -z "$NEW" ] && exit 0
OLD="$(cat "$STATE" 2>/dev/null)"
[ "$NEW" = "$OLD" ] && exit 0

for p in $PORTS; do
    if [ -n "$OLD" ]; then
        firewall-cmd --permanent --remove-rich-rule="rule family=\"ipv4\" source address=\"$OLD\" port port=\"$p\" protocol=\"tcp\" accept" >/dev/null 2>&1
    fi
    firewall-cmd --permanent --add-rich-rule="rule family=\"ipv4\" source address=\"$NEW\" port port=\"$p\" protocol=\"tcp\" accept" >/dev/null 2>&1
done
firewall-cmd --reload >/dev/null 2>&1
mkdir -p "$(dirname "$STATE")"
echo "$NEW" > "$STATE"
logger -t ddns-whitelist "home IP changed: ${OLD:-none} -> $NEW (ports: $PORTS)"
