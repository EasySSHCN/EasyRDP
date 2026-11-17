#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/34b611c" > /dev/null
RUN=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs?head_sha=34b611c&per_page=1" | python3 -c "import json,sys;print(json.load(sys.stdin)['workflow_runs'][0]['id'])")
echo "run id: $RUN"
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/$RUN/jobs" > /tmp/jobs2.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/jobs2.json'))
for j in d.get('jobs', []):
    if j.get('conclusion') == 'failure':
        print(j['id'], j['name'])
EOF
