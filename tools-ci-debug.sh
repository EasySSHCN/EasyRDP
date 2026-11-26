#!/usr/bin/env bash
set -uo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
echo "== runs =="
curl -s -H "Authorization: token $TOKEN" "https://api.github.com/repos/EasySSHCN/EasyRDP/actions/runs?per_page=6" > /tmp/runs.json
python3 - <<'EOF'
import json
for r in json.load(open('/tmp/runs.json')).get('workflow_runs', []):
    print(r['id'], '|', r['name'], '|', r['head_branch'], '|', r['event'], '|', r['status'], '|', r['conclusion'])
EOF
echo "== branch ci/workflow-fix vs main: release.yml diff =="
cd /opt/dsh/workspace/EasyRDP
git fetch -q origin ci/workflow-fix 2>/dev/null || git fetch -q origin
git diff origin/main origin/ci-workflow-fix -- .github/workflows/release.yml | head -60
echo "== tag v0.1.0-ci.1 points to =="
git ls-remote --tags origin | tail -4
