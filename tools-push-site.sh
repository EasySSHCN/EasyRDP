#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "site: real repo/release links, self-hosted source download"
git push -q -f "https://EasySSHCN:${TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags
git push -q -f "https://easyssh:${GITEE}@gitee.com/easyssh/EasyRDP.git" main --tags
echo "site update pushed to both repos"
