#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
git log --oneline ci/workflow-fix | head -3
NEW=$(git rev-parse ci/workflow-fix)
git tag -f v0.1.0-ci.1 $NEW
git push -q -f origin v0.1.0-ci.1
echo "tag v0.1.0-ci.1 -> $NEW ; CI re-triggered"
