#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/contents/.github/workflows/release.yml?ref=6b03bdcb6400cfa592ec28b23a2fb403653742d9" -o /tmp/rel-meta.json
python3 - <<'EOF'
import json, base64
d = json.load(open('/tmp/rel-meta.json'))
open('/tmp/release-fix.yml', 'w').write(base64.b64decode(d['content']).decode('utf-8'))
print('saved', d.get('size'))
EOF
echo "--- secrets usage ---"
grep -n "secrets\." /tmp/release-fix.yml | head -10
echo "--- top-level structure ---"
grep -nE "^(on|jobs|permissions|env|true):|^\s+if:" /tmp/release-fix.yml | head -25
