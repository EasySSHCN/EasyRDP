#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
echo "== check runs for head sha 6b03bdc =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/commits/6b03bdcb6400cfa592ec28b23a2fb403653742d9/check-runs" > /tmp/cr.json
python3 - <<'EOF'
import json
d = json.load(open('/tmp/cr.json'))
print('total:', d.get('total_count'))
for c in d.get('check_runs', []):
    print('run:', c['name'], '|', c['status'], '|', c['conclusion'])
    for a in c.get('output', {}).get('annotations', []) or []:
        print('   annotation:', a.get('message'))
    out = c.get('output', {})
    if out.get('title'): print('   title:', out['title'])
    if out.get('summary'): print('   summary:', out['summary'][:400])
EOF
echo "== run-level details =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/43139311256" > /tmp/r1.json
python3 - <<'EOF'
import json
r = json.load(open('/tmp/r1.json'))
for k in ('name','head_branch','head_sha','event','status','conclusion','display_title'):
    print(k, '=', r.get(k))
EOF
