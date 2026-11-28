#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
# 上一 commit 含 workflows 导致 GitHub 拒绝；拆分：软重置后分两个 commit
git reset -q --soft HEAD~1
git reset -q HEAD .github
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "feat(rdp): native FreeRDP2 FFI implementation behind feature gate; docs: CI guide" || echo "(no changes)"
git push -q -f "https://EasySSHCN:${TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags
git push -q -f "https://easyssh:${GITEE}@gitee.com/easyssh/EasyRDP.git" main --tags
echo "non-workflow changes pushed to both repos"
# workflows 单独提交在本地分支暂存（待 PAT 加 workflow scope 后推送）
git checkout -q -b ci-workflows
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "ci: 3-platform release/CI workflows with optional trusted signing" || echo "(workflows already committed)"
echo "workflow commit staged on branch ci-workflows"
