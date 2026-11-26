#!/usr/bin/env bash
# 最终发布：GitHub 替换全部附件 + 新增网关包；Gitee 发行版重建（5 附件）
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GH_REPO="EasySSHCN/EasyRDP"
GE_REPO="easyssh/EasyRDP"
VER=0.1.0-alpha
FILES=(/tmp/EasyRDP-$VER-portable.exe /tmp/EasyRDP-$VER-win32-x64-portable.zip /tmp/EasyRDP-Gateway-$VER-win-x64.zip /tmp/EasyRDP-src-$VER.tar.gz /tmp/SHA256SUMS.txt)

echo "== GitHub: replace assets =="
REL_ID=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/tags/v$VER" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
for N in EasyRDP-$VER-portable.exe EasyRDP-$VER-win32-x64-portable.zip EasyRDP-Gateway-$VER-win-x64.zip EasyRDP-src-$VER.tar.gz SHA256SUMS.txt; do
  AID=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/$REL_ID/assets" | python3 -c "
import json,sys
for a in json.load(sys.stdin):
    if a.get('name')=='$N':
        print(a['id']); break")
  [ -n "$AID" ] && curl -s -X DELETE -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/assets/$AID" -o /dev/null
done
for f in "${FILES[@]}"; do
  [ -f "$f" ] || continue
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @"$f" \
    "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=$(basename "$f")" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GH',d.get('name'),'=',d.get('state') or d.get('message'))"
done

echo "== Gitee: rebuild release =="
OLD_GID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
[ -n "$OLD_GID" ] && curl -s -X DELETE "https://gitee.com/api/v5/repos/$GE_REPO/releases/$OLD_GID?access_token=$GITEE" -o /dev/null -w "  old deleted http %{http_code}\n"
README=/opt/dsh/workspace/EasyRDP/README.md
NOTES=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
BODY="## EasyRDP v$VER

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新
$NOTES"
NOTES_JSON=$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")
GREL=$(mktemp)
curl -s -X POST -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/$GE_REPO/releases" \
  -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"v$VER\",\"target_commitish\":\"main\",\"name\":\"EasyRDP v$VER\",\"body\":$NOTES_JSON,\"prerelease\":true}" > "$GREL"
GID=$(python3 -c "import json;d=json.load(open('$GREL'));print(d.get('id','') or d.get('original_id','') if isinstance(d,dict) else '')")
echo "  new release id=$GID"
rm -f "$GREL"
for f in "${FILES[@]}"; do
  [ -f "$f" ] || continue
  curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GE',d.get('name') or d.get('message'))" | head -1
done
echo "done"
