#!/usr/bin/env bash
# gitee-quota-check.sh — 统计各 Release 附件占用
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
BASE="https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases"
PAGE=1
while :; do
  LIST=$(curl -s "$BASE?access_token=$GE&page=$PAGE&per_page=50&direction=desc")
  IDS=$(printf '%s' "$LIST" | python3 -c "import json,sys;[print(r['id']) for r in json.load(sys.stdin)]" 2>/dev/null) || IDS=""
  [ -z "$IDS" ] && break
  for GID in $IDS; do
    curl -s "$BASE/$GID?access_token=$GE" -o /tmp/rel.json
    TAG=$(python3 -c "import json;print(json.load(open('/tmp/rel.json')).get('tag_name',''))")
    python3 - "$TAG" <<'EOF'
import json,sys
d=json.load(open('/tmp/rel.json'))
tot=sum(a.get('size') or 0 for a in (d.get('assets') or []))
if tot or (d.get('assets')):
    print(f"{d.get('tag_name')} assets={len(d.get('assets') or [])} total={tot/1048576:.1f}MB")
EOF
  done
  PAGE=$((PAGE+1)); [ "$PAGE" -gt 8 ] && break
done
