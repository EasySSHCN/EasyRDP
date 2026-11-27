#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
RUN=43141692486
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs/$RUN/jobs" > /tmp/j4.json
for JID in $(python3 -c "
import json
for j in json.load(open('/tmp/j4.json'))['jobs']:
    if j.get('conclusion')=='failure': print(j['id'])"); do
  curl -s -L -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/jobs/$JID/logs" -o /tmp/log-$JID.txt
  echo "--- job $JID: bundle ls 与错误 ---"
  grep -A 12 "No .dmg found\|No .AppImage found" /tmp/log-$JID.txt | head -25
done
