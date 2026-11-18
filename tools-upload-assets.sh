#!/usr/bin/env bash
# 上传 Windows 产物到 GitHub Release 与 Gitee Release 附件
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GH_REPO="EasySSHCN/EasyRDP"
GE_REPO="easyssh/EasyRDP"
VER=0.1.0-alpha

echo "== GitHub Release assets =="
REL_ID=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/tags/v$VER" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
echo "release id=$REL_ID"
for f in /tmp/EasyRDP-$VER-portable.exe /tmp/EasyRDP-$VER-win32-x64-portable.zip /tmp/SHA256SUMS.txt; do
  [ -f "$f" ] || continue
  N=$(basename "$f")
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @"$f" \
    "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=$N" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print(' ',d.get('name'),'=',d.get('state') or d.get('message'))"
done

echo "== Gitee Release assets =="
GID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
echo "release id=$GID"
for f in /tmp/EasyRDP-$VER-portable.exe /tmp/EasyRDP-$VER-win32-x64-portable.zip /tmp/SHA256SUMS.txt; do
  [ -f "$f" ] || continue
  curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print(' ',d.get('name') or d.get('message'))" | head -1
done
echo "done"
