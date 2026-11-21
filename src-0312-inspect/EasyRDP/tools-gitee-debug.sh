#!/usr/bin/env bash
set -uo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GE_REPO="easyssh/EasyRDP"
echo "== repo info =="
curl -s "https://gitee.com/api/v5/repos/${GE_REPO}?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);print('path:',d.get('full_name'),'private:',d.get('private'),'default_branch:',d.get('default_branch'))"
echo "== tags =="
curl -s "https://gitee.com/api/v5/repos/${GE_REPO}/tags?access_token=$GITEE" | python3 -c "import json,sys;print([t.get('name') for t in json.load(sys.stdin)])"
echo "== create release (retry with explicit name field) =="
BODY=$(python3 -c "import json;print(json.dumps({'access_token':'','tag_name':'v0.1.0-alpha','target_commitish':'main','name':'EasyRDP v0.1.0-alpha','body':'first','prerelease':True}))")
curl -s -X POST -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/${GE_REPO}/releases" \
  -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"v0.1.0-alpha\",\"target_commitish\":\"main\",\"name\":\"EasyRDP v0.1.0-alpha\",\"body\":\"first\",\"prerelease\":true}" | head -c 400
echo
