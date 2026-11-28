#!/usr/bin/env bash
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases?access_token=$GE&page=1&per_page=50&direction=desc" -o /tmp/rels.json
python3 - <<'EOF'
import json
for r in json.load(open('/tmp/rels.json')):
    names=[a.get('name') for a in (r.get('assets') or [])]
    print(r.get('tag_name'), r.get('id'), names)
EOF
