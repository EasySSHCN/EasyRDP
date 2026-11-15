#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
git fetch -q origin ci/workflow-fix
git checkout -q ci/workflow-fix
F=.github/workflows/release.yml
# 1) step 级 if 里的 secrets 改为 env 映射
python3 - <<'EOF'
import re
p = '.github/workflows/release.yml'
c = open(p, encoding='utf-8').read()
c = c.replace("if: matrix.platform == 'windows' && secrets.AZURE_SIGNING_ACCOUNT != ''",
              "if: matrix.platform == 'windows' && env.AZURE_SIGNING_ACCOUNT != ''")
open(p, 'w', encoding='utf-8').write(c)
print('step-if fixed:', "env.AZURE_SIGNING_ACCOUNT" in c)
EOF
# 2) 确保顶层 env 映射存在
if ! grep -q "AZURE_SIGNING_ACCOUNT: \${{ secrets.AZURE_SIGNING_ACCOUNT }}" $F; then
  python3 - <<'EOF'
p = '.github/workflows/release.yml'
c = open(p, encoding='utf-8').read()
c = c.replace("env:\n", "env:\n  AZURE_SIGNING_ACCOUNT: ${{ secrets.AZURE_SIGNING_ACCOUNT }}\n", 1)
open(p, 'w', encoding='utf-8').write(c)
print('top env added')
EOF
fi
grep -n "AZURE_SIGNING_ACCOUNT" $F
# 3) 提交推送分支
git add $F
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "fix(ci): secrets context not allowed in step if; map via job env"
git push -q origin ci/workflow-fix
# 4) 重打 tag 到新 commit 并强推触发
NEW=$(git rev-parse HEAD)
git tag -f v0.1.0-ci.1 $NEW
git push -q -f origin v0.1.0-ci.1
echo "tag moved to $NEW; run: https://github.com/EasySSHCN/EasyRDP/actions"
