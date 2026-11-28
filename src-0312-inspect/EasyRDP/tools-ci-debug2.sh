#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
echo "== jobs of run 43139311256 (release @ ci/workflow-fix) =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/43139311256/jobs" > /tmp/j1.json
python3 - <<'EOF'
import json
for j in json.load(open('/tmp/j1.json')).get('jobs', []):
    print(j['name'], '|', j['status'], '|', j['conclusion'])
    for s in j.get('steps', [])[:3]:
        print('   step:', s['name'], s['conclusion'])
EOF
echo "== jobs of run 43139104299 (release @ main) =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/43139104299/jobs" > /tmp/j2.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/j2.json'))
print('total_jobs:', d.get('total_count'))
for j in d.get('jobs', []):
    print(j['name'], '|', j['conclusion'])
EOF
echo "== ci.yml run 43133360297 =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/43133360297/jobs" > /tmp/j3.json
python3 - <<'EOF'
import json
for j in json.load(open('/tmp/j3.json')).get('jobs', []):
    print(j['name'], '|', j['conclusion'])
EOF
echo "== fetch release.yml from ci/workflow-fix =="
mkdir -p /tmp/wfchk && cd /tmp/wfchk
git init -q . 2>/dev/null
git remote add origin "https://x:${TOKEN}@github.com/EasySSHCN/EasyRDP.git" 2>/dev/null || true
git fetch -q origin ci/workflow-fix main
echo "-- release.yml (ci/workflow-fix) head --"
git show FETCH_HEAD:.github/workflows/release.yml 2>/dev/null | head -5 || echo "not on that branch tip"
