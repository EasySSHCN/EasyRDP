#!/usr/bin/env bash
T=$(curl -s -X POST "https://easyrdp.cn/api/download/token.php" -H 'Referer: https://easyrdp.cn/download.html' -d 'file=EasyRDP-src-0.4.1.tar.gz')
echo "$T"
U=$(echo "$T" | python3 -c 'import json,sys;print(json.load(sys.stdin)["url"])')
echo "url: $U"
curl -sL -o /dev/null -w "follow -> %{http_code} type=%{content_type} len=%{size_download}\n" "https://easyrdp.cn$U"
curl -sL -r 0-100 "https://easyrdp.cn$U" | head -c 200; echo
