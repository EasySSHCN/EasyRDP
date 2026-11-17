#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
RUN=43140539019
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/$RUN/jobs" > /tmp/j3.json
python3 - <<'EOF'
import json
for j in json.load(open('/tmp/j3.json')).get('jobs', []):
    if j.get('conclusion') == 'failure':
        print(j['id'], j['name'])
EOF
echo "== ubuntu job log tail =="
for JID in $(python3 -c "
import json
for j in json.load(open('/tmp/j3.json'))['jobs']:
    if j.get('conclusion')=='failure': print(j['id'])"); do
  curl -s -L -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/jobs/$JID/logs" -o /tmp/log-$JID.txt
  echo "--- job $JID log tail ---"
  tail -40 /tmp/log-$JID.txt | grep -iE "error|failed|denied|not found|Errno|warning: unused" | head -15
  echo "--- last 8 lines ---"
  tail -8 /tmp/log-$JID.txt
done
