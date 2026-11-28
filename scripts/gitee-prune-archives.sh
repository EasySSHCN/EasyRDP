#!/usr/bin/env bash
# gitee-prune-archives.sh — 删除旧 Release 的自动源码归档附件（vX.zip / vX.tar.gz），释放附件配额
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
BASE="https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases"
PAGE=1; DEL=0
while :; do
  LIST=$(curl -s "$BASE?access_token=$GE&page=$PAGE&per_page=50&direction=desc")
  IDS=$(printf '%s' "$LIST" | python3 -c "import json,sys;[print(r['id']) for r in json.load(sys.stdin)]" 2>/dev/null) || IDS=""
  [ -z "$IDS" ] && break
  for GID in $IDS; do
    curl -s "$BASE/$GID?access_token=$GE" -o /tmp/rel.json
    python3 - "$GID" "$GE" <<'EOF' > /tmp/del-list.txt
import json,sys,os
gid,ge=sys.argv[1],sys.argv[2]
d=json.load(open('/tmp/rel.json'))
tag=d.get('tag_name','')
if tag=='v0.6.1': sys.exit(0)
for a in (d.get('assets') or []):
    n=a.get('name','')
    if n==f'{tag}.zip' or n==f'{tag}.tar.gz':
        print(a['id'], n)
EOF
    while read -r AID NAME; do
      [ -z "$AID" ] && continue
      CODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "$BASE/$GID/assets/$AID?access_token=$GE")
      echo "  删除 $NAME (aid=$AID) http $CODE"
      DEL=$((DEL+1))
    done < /tmp/del-list.txt
  done
  PAGE=$((PAGE+1)); [ "$PAGE" -gt 8 ] && break
done
echo "== 归档清理完成，删除数=$DEL =="
