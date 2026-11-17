#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
git fetch -q origin ci/workflow-fix main
git checkout -q -B main origin/main
git merge -q --ff-only origin/ci/workflow-fix 2>/dev/null || git merge -q origin/ci/workflow-fix -m "merge: remove deb/rpm from release artifacts (portable-only)"
git push -q origin main
echo "main now at $(git rev-parse --short HEAD) (deb/rpm removal merged)"
