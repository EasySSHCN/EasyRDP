#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/EasyRDP
# 用修复后的源码覆盖（tar 内不含 target/dist/test-tmp）
tar -xzf /tmp/client-src-fix.tar.gz
rm -rf easyrdp-client/test-tmp
# 确保 .gitignore 覆盖构建产物
cat >> .gitignore <<'EOF'
easyrdp-client/target/
easyrdp-client/dist/
easyrdp-client/crates/*/test-tmp/
EOF
sort -u .gitignore -o .gitignore
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "fix(client): rdp utf16 encode/decode detection, resolution semantics, name roundtrip; frontend layout; tauri icon; bump 0.1.0-alpha" || echo "(no changes)"
git push -q -f "https://EasySSHCN:${TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags
git push -q -f "https://easyssh:${GITEE}@gitee.com/easyssh/EasyRDP.git" main --tags
echo "pushed to both repos"
git log --oneline | head -2
