#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
git fetch -q origin main
git checkout -q -B release-conf origin/main
tar -xzf /tmp/conf-fix.tar.gz
git add easyrdp-client/src-tauri/tauri.conf.json
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "fix(bundle): enable bundle config (active/targets/icons) and rename identifier to cn.easyrdp.client — root cause of missing dmg/AppImage"
git push -q origin HEAD:main
NEW=$(git rev-parse HEAD)
git tag -f v0.1.0-ci.3 $NEW
git push -q -f origin v0.1.0-ci.3
echo "conf fix pushed to main; tag v0.1.0-ci.3 -> $NEW"
