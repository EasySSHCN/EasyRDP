#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
# 备份 workflow 分支到 Gitee（GitHub 侧待 PAT workflow scope 后推送 main）
git push -q -f "https://easyssh:${GITEE}@gitee.com/easyssh/EasyRDP.git" ci-workflows && echo "ci-workflows branch backed up to Gitee"
git log --oneline ci-workflows | head -2
