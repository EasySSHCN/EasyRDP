#!/usr/bin/env bash
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
GE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key" | sed -n 's/^gitee_api_token: //p')
curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/tags/v0.5.1?access_token=$GE" -o /tmp/gerel.json
python3 - <<'EOF'
import json
j = json.load(open('/tmp/gerel.json'))
print('tag:', j.get('tag_name'), 'id:', j.get('id'))
for a in j.get('assets', []) or []:
    print(' asset:', a.get('name'), a.get('size'))
EOF
