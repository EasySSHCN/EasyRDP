#!/usr/bin/env bash
set -uo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GE_REPO="easyssh/EasyRDP"
VER=0.1.0-alpha
GID=1298980

echo "== 1. set public (PATCH with name) =="
curl -s -X PATCH -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/${GE_REPO}" \
  -d "{\"access_token\":\"$GITEE\",\"name\":\"EasyRDP\",\"private\":false}" | python3 -c "import json,sys;d=json.load(sys.stdin);print('private:',d.get('private'),'| msg:',d.get('message'))" 2>/dev/null || true

echo "== 2. update release (PATCH) =="
README=/opt/dsh/workspace/EasyRDP/README.md
NOTES=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
BODY="## EasyRDP v$VER

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新
$NOTES"
NOTES_JSON=$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")
curl -s -X PATCH -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/${GE_REPO}/releases/$GID" \
  -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"v$VER\",\"name\":\"EasyRDP v$VER\",\"body\":$NOTES_JSON,\"prerelease\":true}" \
  | python3 -c "import json,sys;d=json.load(sys.stdin);print('release:',d.get('name'),'| body_len:',len(d.get('body','')),'| msg:',d.get('message'))"

echo "== verify =="
curl -s "https://gitee.com/api/v5/repos/${GE_REPO}?access_token=$GITEE" | python3 -c "import json,sys;print('repo private:',json.load(sys.stdin).get('private'))"
curl -s "https://gitee.com/api/v5/repos/${GE_REPO}/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);print('release name:',d.get('name'),'| body head:',d.get('body','')[:60].replace(chr(10),' '))"
