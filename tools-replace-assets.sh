#!/usr/bin/env bash
# 替换 GitHub/Gitee Release 上的 Windows 产物（修复 dev 模式的重新构建版）
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GH_REPO="EasySSHCN/EasyRDP"
GE_REPO="easyssh/EasyRDP"
REL_ID=441875230

echo "== GitHub: delete old assets =="
for N in EasyRDP-0.1.0-alpha-portable.exe EasyRDP-0.1.0-alpha-win32-x64-portable.zip SHA256SUMS.txt; do
  AID=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/$REL_ID/assets" | python3 -c "
import json,sys
for a in json.load(sys.stdin):
    if a.get('name')=='$N':
        print(a['id']); break")
  [ -n "$AID" ] && curl -s -X DELETE -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/assets/$AID" -o /dev/null -w "  deleted $N http %{http_code}\n"
done
echo "== GitHub: upload new =="
for f in /tmp/EasyRDP-0.1.0-alpha-portable.exe /tmp/EasyRDP-0.1.0-alpha-win32-x64-portable.zip /tmp/SHA256SUMS.txt; do
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @"$f" \
    "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=$(basename "$f")" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print('  ',d.get('name'),'=',d.get('state') or d.get('message'))"
done

echo "== Gitee: delete old assets =="
GID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/tags/v0.1.0-alpha?access_token=$GITEE" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
for N in EasyRDP-0.1.0-alpha-portable.exe EasyRDP-0.1.0-alpha-win32-x64-portable.zip SHA256SUMS.txt; do
  AID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/assets?access_token=$GITEE" | python3 -c "
import json,sys
for a in json.load(sys.stdin):
    if a.get('name')=='$N':
        print(a['id']); break")
  [ -n "$AID" ] && curl -s -X DELETE "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/assets?access_token=$GITEE&asset_id=$AID" -o /dev/null -w "  deleted $N http %{http_code}\n"
done
echo "== Gitee: upload new =="
for f in /tmp/EasyRDP-0.1.0-alpha-portable.exe /tmp/EasyRDP-0.1.0-alpha-win32-x64-portable.zip /tmp/SHA256SUMS.txt; do
  curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  ',d.get('name') or d.get('message'))" | head -1
done
echo "done"
