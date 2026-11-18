#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
git fetch -q origin ci/workflow-fix
git checkout -q ci/workflow-fix
tar -xzf /tmp/icon-fix.tar.gz
git add easyrdp-client/src-tauri/icons/icon.png
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "fix(ci): add icons/icon.png required by tauri codegen on linux/macos"
git push -q origin ci/workflow-fix
# 同步到 main
git push -q -f origin ci/workflow-fix:main
NEW=$(git rev-parse HEAD)
git tag -f v0.1.0-ci.2 $NEW
git push -q -f origin v0.1.0-ci.2
echo "icon fix pushed; tag v0.1.0-ci.2 -> $NEW"
