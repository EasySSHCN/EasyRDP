#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
for i in $(seq 1 60); do
  sleep 30
  R=$(curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs?per_page=1" | python3 -c "
import json,sys
r = json.load(sys.stdin)['workflow_runs'][0]
print(r['id'], r['head_sha'][:7], r['status'], r['conclusion'])")
  echo "poll $i: $R"
  case "$R" in *completed*) break;; esac
done
RID=$(echo $R | awk '{print $1}')
echo "== jobs of run $RID =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/$RID/jobs" > /tmp/jobs.json
python3 - <<'EOF'
import json
for j in json.load(open('/tmp/jobs.json')).get('jobs', []):
    print(j['name'], '|', j['status'], '|', j['conclusion'])
    for s in j.get('steps', []):
        if s['conclusion'] not in ('skipped', None, 'success'):
            print('   !', s['name'], '->', s['conclusion'])
EOF
