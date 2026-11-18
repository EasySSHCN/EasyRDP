#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/contents/.github/workflows/release.yml?ref=6b03bdcb6400cfa592ec28b23a2fb403653742d9" | python3 - <<'EOF'
import json,sys,base64
d = json.load(sys.stdin)
open('/tmp/release-fix.yml','w').write(base64.b64decode(d['content']).decode('utf-8'))
print('saved', d.get('size'))
EOF
grep -n "secrets\." /tmp/release-fix.yml | head -10
echo "--- on/jobs structure ---"
grep -nE "^(on|jobs|permissions|env):|^  [a-z_]+:$|if:" /tmp/release-fix.yml | head -20
