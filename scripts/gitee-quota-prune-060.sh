#!/usr/bin/env bash
# Gitee 附件配额清理 v2：逐 Release 详情解析 assets，删除 <v0.6.0 的非 SHA256SUMS、非自动源码归档附件
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
BASE="https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases"
PAGE=1; TOTAL_DELETED=0
while :; do
  LIST=$(curl -s "$BASE?access_token=$GE&page=$PAGE&per_page=50&direction=desc")
  IDS=$(printf '%s' "$LIST" | python3 -c "import json,sys;[print(r['id']) for r in json.load(sys.stdin)]" 2>/dev/null) || IDS=""
  [ -z "$IDS" ] && break
  for GID in $IDS; do
    DETAIL=$(curl -s "$BASE/$GID?access_token=$GE")
    TAG=$(printf '%s' "$DETAIL" | python3 -c "import json,sys;print(json.load(sys.stdin).get('tag_name',''))" 2>/dev/null)
    [ -z "$TAG" ] && continue
    echo "--- $TAG (id=$GID)"
    if [ "$TAG" = "v0.6.1" ]; then echo "  保留（当前版本）"; continue; fi
    printf '%s' "$DETAIL" | python3 -c "
import json,sys
for a in (json.load(sys.stdin).get('assets') or []):
    n = a.get('name','')
    if 'SHA256SUMS' in n: continue
    if n in ('v0.6.0.zip','v0.6.0.tar.gz') or n.startswith('v') and (n.endswith('.zip') or n.endswith('.tar.gz')): continue
    print(a['id'], n)
" 2>/dev/null | while read -r AID NAME; do
      CODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "$BASE/$GID/assets/$AID?access_token=$GE")
      echo "  删除 $NAME (aid=$AID) http $CODE"
    done
  done
  PAGE=$((PAGE+1))
  [ "$PAGE" -gt 5 ] && break
done
echo "== 清理完成 =="
