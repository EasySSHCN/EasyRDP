#!/usr/bin/env bash
set -uo pipefail
python3 - <<'EOF'
import json
d = json.load(open('/tmp/rel.json'))
assets = d.get('assets', [])
print("total:", len(assets))
print(json.dumps(assets[3], ensure_ascii=False))
EOF
