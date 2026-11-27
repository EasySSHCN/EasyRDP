#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GH_REPO="EasySSHCN/EasyRDP"
REL_ID=441875230
echo "== delete old SHA256SUMS.txt asset =="
AID=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/$REL_ID/assets" | python3 -c "
import json,sys
for a in json.load(sys.stdin):
    if a.get('name')=='SHA256SUMS.txt':
        print(a['id']); break")
echo "asset id=$AID"
[ -n "$AID" ] && curl -s -X DELETE -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/assets/$AID" -o /dev/null -w "deleted http %{http_code}\n"
echo "== upload combined SHA256SUMS.txt =="
curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
  --data-binary @/tmp/SHA256SUMS.txt \
  "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=SHA256SUMS.txt" \
  | python3 -c "import json,sys;d=json.load(sys.stdin);print(' ',d.get('name'),'=',d.get('state') or d.get('message'))"
echo "== final asset list =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/$GH_REPO/releases/$REL_ID/assets" | python3 -c "
import json,sys
for a in json.load(sys.stdin):
    print(' ', a['name'], a['size'])"
