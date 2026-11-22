#!/usr/bin/env bash
set -uo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GE_REPO="easyssh/EasyRDP"
GID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/tags/v0.1.0-alpha?access_token=$GITEE" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
echo "release id=$GID"
echo "== raw assets response =="
curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/assets?access_token=$GITEE" | head -c 600
echo
echo "== raw release detail assets =="
curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID?access_token=$GITEE" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for a in d.get('assets',[]):
    print(' ', a.get('name'), a.get('size'))"
