#!/usr/bin/env bash
set -uo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GE_REPO="easyssh/EasyRDP"
VER=0.1.0-alpha
GID=1298980
SRC=/tmp/EasyRDP-src-0.1.0-alpha.tar.gz

echo "== 1. try set public (JSON PATCH) =="
curl -s -X PATCH -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/${GE_REPO}" \
  -d "{\"access_token\":\"$GITEE\",\"private\":false}" | head -c 200
echo
curl -s "https://gitee.com/api/v5/repos/${GE_REPO}?access_token=$GITEE" | python3 -c "import json,sys;print('private now:',json.load(sys.stdin).get('private'))"

echo "== 2. update release body =="
README=/opt/dsh/workspace/EasyRDP/README.md
NOTES=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
BODY="## EasyRDP v$VER

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新
$NOTES"
NOTES_JSON=$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")
curl -s -X PUT -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/${GE_REPO}/releases/$GID" \
  -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"v$VER\",\"name\":\"EasyRDP v$VER\",\"body\":$NOTES_JSON,\"prerelease\":true}" \
  | python3 -c "import json,sys;d=json.load(sys.stdin);print('release updated:',d.get('name'),'body_len:',len(d.get('body','')))"

echo "== 3. attach files =="
curl -s -X POST "https://gitee.com/api/v5/repos/${GE_REPO}/releases/$GID/attach_files?access_token=$GITEE" \
  -F "file=@$SRC" | python3 -c "import json,sys;d=json.load(sys.stdin);print('附件:',d.get('name') or d.get('message'))" | head -1
curl -s -X POST "https://gitee.com/api/v5/repos/${GE_REPO}/releases/$GID/attach_files?access_token=$GITEE" \
  -F "file=@/tmp/EasyRDP-src-SHA256SUMS.txt" | python3 -c "import json,sys;d=json.load(sys.stdin);print('附件:',d.get('name') or d.get('message'))" | head -1
