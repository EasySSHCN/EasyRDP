#!/usr/bin/env bash
# gh-ge-fix-051.sh —— v0.5.1 资产修复：GitHub 替换附件 / Gitee 删除重建
set -uo pipefail
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
DIST="$ROOT/easyrdp-client/dist"
VAULT="$ROOT/secrets-vault"
VER=0.5.1
TAG="v$VER"
PLAIN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/creds.enc" -pass file:"$VAULT/key")
GH_TOKEN=$(printf '%s\n' "$PLAIN" | sed -n 's/^github_api_token: //p')
GE_TOKEN=$(printf '%s\n' "$PLAIN" | sed -n 's/^gitee_api_token: //p')

EXE="$DIST/EasyRDP-$VER-portable.exe"
ZIP="$DIST/EasyRDP-$VER-win32-x64-portable.zip"
GW="$ROOT/dist-gateway/EasyRDP-Gateway-$VER-win-x64.zip"
SRC="$ROOT/EasyRDP-src-$VER.tar.gz"
SUMS="$DIST/SHA256SUMS.txt"

echo "==> GitHub v$VER 附件全量替换"
REL_ID=$(curl -s -H "Authorization: token $GH_TOKEN" \
  "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/tags/$TAG" \
  | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
[ -n "$REL_ID" ] && echo "  release id=$REL_ID"
for A in $(curl -s -H "Authorization: token $GH_TOKEN" \
    "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/$REL_ID/assets" \
    | python3 -c "
import json,sys
for a in json.load(sys.stdin): print(a['id'])"); do
  curl -s -X DELETE -H "Authorization: token $GH_TOKEN" \
    "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/assets/$A" -o /dev/null
done
sleep 2
UP=$(curl -s -H "Authorization: token $GH_TOKEN" \
  "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/$REL_ID" \
  | python3 -c "import json,sys;print(json.load(sys.stdin).get('upload_url','').split('{')[0])")
for F in "$EXE" "$ZIP" "$GW" "$SRC" "$SUMS"; do
  N=$(basename "$F")
  CT=$([ "$N" == "${N%.zip}" ] && [ "$N" == "${N%.gz}" ] && echo "application/octet-stream" || echo "application/zip")
  R=$(curl -s -X POST -H "Authorization: token $GH_TOKEN" -H "Content-Type: $CT" \
    --data-binary @"$F" --max-time 3600 "$UP?name=$N" \
    | python3 -c "import json,sys;j=json.load(sys.stdin);print(j.get('state') or j)")
  echo "  GH $N = $R"
done

echo "==> Gitee v$VER Release 删除重建"
REL=$(curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/tags/$TAG?access_token=$GE_TOKEN")
GID=$(printf '%s' "$REL" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
if [ -n "$GID" ]; then
  curl -s -X DELETE "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID?access_token=$GE_TOKEN" -o /dev/null
  echo "  旧 release id=$GID 已删除"
fi
NOTES=$(awk -v ver="### v$VER" 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$ROOT/README.md")
BODY="## EasyRDP $TAG

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新

$NOTES"
BODY_JSON=$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")
NEW=$(curl -s -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases" \
  -d "access_token=$GE_TOKEN" -d "tag_name=$TAG" -d "target_commitish=main" \
  -d "name=EasyRDP $TAG" --data-urlencode "body=$BODY")
NEWID=$(printf '%s' "$NEW" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
echo "  新 release id=$NEWID"
for F in "$EXE" "$ZIP" "$GW" "$SRC" "$SUMS"; do
  N=$(basename "$F")
  R=$(curl -s -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$NEWID/attach_files" \
    -d "access_token=$GE_TOKEN" -F "file=@$F" --max-time 3600 \
    | python3 -c "import json,sys;j=json.load(sys.stdin);print(j.get('browser_download_url') or j)")
  echo "  GE $N = $R"
done
echo DONE
