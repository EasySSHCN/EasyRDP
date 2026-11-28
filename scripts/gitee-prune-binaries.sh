#!/usr/bin/env bash
# gitee-prune-binaries.sh — 删除 v0.6.0 以前 Release 的二进制附件（保留 SHA256SUMS 与自动归档）
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
BASE="https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases"
PAGE=1; DEL=0; FAIL=0
while :; do
  LIST=$(curl -s "$BASE?access_token=$GE&page=$PAGE&per_page=50&direction=desc")
  IDS=$(printf '%s' "$LIST" | python3 -c "import json,sys;[print(r['id']) for r in json.load(sys.stdin)]" 2>/dev/null) || IDS=""
  [ -z "$IDS" ] && break
  for GID in $IDS; do
    curl -s "$BASE/$GID/attach_files?access_token=$GE" -o /tmp/assets.json
    python3 - "$GID" <<'EOF' > /tmp/del-list.txt
import json,sys
gid=sys.argv[1]
try:
    assets=json.load(open('/tmp/assets.json'))
except Exception:
    sys.exit(0)
if not isinstance(assets,list): sys.exit(0)
tag=''
for a in assets:
    n=a.get('name','')
    if 'SHA256SUMS' in n: continue
    if n.startswith('v') and (n.endswith('.zip') or n.endswith('.tar.gz')): continue
    aid=a.get('id')
    if aid: print(aid, n)
EOF
    while read -r AID NAME; do
      [ -z "$AID" ] && continue
      CODE=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "$BASE/$GID/attach_files/$AID?access_token=$GE")
      echo "  [$GID] 删除 $NAME (aid=$AID) http $CODE"
      if [ "$CODE" = "200" ] || [ "$CODE" = "204" ]; then DEL=$((DEL+1)); else FAIL=$((FAIL+1)); fi
    done < /tmp/del-list.txt
  done
  PAGE=$((PAGE+1)); [ "$PAGE" -gt 8 ] && break
done
echo "== 删除=$DEL 失败=$FAIL =="
