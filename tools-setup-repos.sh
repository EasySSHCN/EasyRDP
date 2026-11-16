#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
echo "== GitHub account =="
GH_LOGIN=$(curl -s -H "Authorization: token $TOKEN" https://api.github.com/user | grep -m1 '"login"' | sed 's/.*: "//;s/",*//')
echo "GH_LOGIN=$GH_LOGIN"
echo "== Gitee account =="
GE_LOGIN=$(curl -s "https://gitee.com/api/v5/user?access_token=$GITEE" | grep -m1 '"login"' | sed 's/.*: "//;s/",*//')
echo "GE_LOGIN=$GE_LOGIN"
echo "== create GitHub repo =="
curl -s -X POST -H "Authorization: token $TOKEN" https://api.github.com/user/repos \
  -d '{"name":"EasyRDP","description":"EasyRDP - 跨平台开源远程桌面客户端 (Rust + Tauri 2 + FreeRDP)。官网: https://easyrdp.cn","has_issues":true,"has_wiki":false}' \
  | grep -E '"full_name"|"message"' | head -3
echo "== create Gitee repo =="
curl -s -X POST "https://gitee.com/api/v5/user/repos" \
  -d "access_token=$GITEE" -d "name=EasyRDP" \
  -d "description=EasyRDP - 跨平台开源远程桌面客户端 (Rust + Tauri 2 + FreeRDP)。官网: https://easyrdp.cn" \
  -d "has_issues=true" | grep -E '"full_name"|"message"' | head -3
