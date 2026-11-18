#!/usr/bin/env bash
set -e
# 用 UTF-8 显式编码的临时文件发请求（避免 PS 编码污染）
cat > /tmp/report-body.json <<'BODY'
{"action":"report","anonId":"497a22fefc62410033708af9faaf1465c065b6e093d36b365e94af900cbd9479","fields":{"软件版本":"0.3.3","操作系统":"Windows 11","系统架构":"x86_64","客户端类型":"easyrdp-desktop"}}
BODY
curl -s -X POST "https://easyrdp.cn/api/stats/client.php" \
  -H "Content-Type: application/json; charset=utf-8" \
  --data-binary "@/tmp/report-body.json"
echo
rm -f /tmp/report-body.json
