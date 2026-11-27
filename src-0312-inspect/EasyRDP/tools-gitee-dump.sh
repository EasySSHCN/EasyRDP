#!/usr/bin/env bash
set -uo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/1298980?access_token=$GITEE" > /tmp/rel.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/rel.json'))
for a in d.get('assets', []):
    keys = {k: a[k] for k in ('id','name','size','browser_download_url') if k in a}
    print(keys)
EOF
