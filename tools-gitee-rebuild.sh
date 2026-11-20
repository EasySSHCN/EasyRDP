#!/usr/bin/env bash
set -euo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GE_REPO="easyssh/EasyRDP"
GID=1298980
VER=0.1.0-alpha

echo "== delete old release =="
curl -s -X DELETE "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID?access_token=$GITEE" -o /dev/null -w "http %{http_code}\n"

echo "== recreate release =="
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
echo "new release id=$GID"
rm -f "$GREL"

echo "== upload final assets (4 files) =="
for f in /tmp/EasyRDP-$VER-portable.exe /tmp/EasyRDP-$VER-win32-x64-portable.zip /tmp/EasyRDP-src-$VER.tar.gz /tmp/SHA256SUMS.txt; do
  [ -f "$f" ] || continue
  curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  ',d.get('name') or d.get('message'))" | head -1
done

echo "== verify asset list =="
curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID?access_token=$GITEE" > /tmp/rel2.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/rel2.json'))
for a in d.get('assets', []):
    print('  ', a.get('name'))
EOF
