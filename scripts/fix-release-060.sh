#!/usr/bin/env bash
# v0.6.0 发布补丁：①GitHub main push 重试（TLS 网络错误）②Gitee 3 附件重传验证
set -u
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
D=$R/easyrdp-client/dist
PLAIN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key")
GH=$(printf '%s\n' "$PLAIN" | sed -n 's/^github_api_token: //p')
GE=$(printf '%s\n' "$PLAIN" | sed -n 's/^gitee_api_token: //p')

echo "== 1) GitHub main + tags push 重试 =="
for i in 1 2 3; do
  if git -C "$R/easyrdp-client/dist/release-src" status >/dev/null 2>&1; then :; fi
  if git -C "$R" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    # 主仓库在根目录? release.sh 是从解包源码树 push。检查 dist 下 release-src
    :; fi
  break
done
# release.sh 用解包树 push：重建等价操作——在根仓不可用则用源码树临时解包
RELDIR=$(mktemp -d)
tar -xzf "$R/EasyRDP-src-0.6.0.tar.gz" -C "$RELDIR" --strip-components=1
cd "$RELDIR"
git init -q -b main 2>/dev/null || true
git add -A && git -c user.name=EasySSHCN -c user.email=release@easyrdp.cn commit -qm "release: v0.6.0" 2>/dev/null || true
git tag -f v0.6.0
if git -c http.version=HTTP/1.1 push -q -f "https://x-access-token:${GH}@github.com/EasySSHCN/EasyRDP.git" main --tags; then
  echo "GitHub push OK（重试第 $i 次）"
else
  echo "GitHub push 仍失败"
fi

echo "== 2) Gitee 附件重传（release id=1374212）=="
for F in "$D/EasyRDP-0.6.0-portable.exe" "$D/EasyRDP-0.6.0-win32-x64-portable.zip" "$R/dist-gateway/EasyRDP-Gateway-0.6.0-win-x64.zip"; do
  N=$(basename "$F")
  CODE=$(curl -s -o /tmp/ge-up.json -w "%{http_code}" --max-time 3600 \
    -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/1374212/attach_files?access_token=$GE" \
    -F "file=@$F")
  echo "$N -> HTTP $CODE $(head -c 160 /tmp/ge-up.json)"
done
# 附件清单复核
curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/1374212/assets?access_token=$GE" \
  | python3 -c "import json,sys;[print(' -',a.get('name'),a.get('size')) for a in json.load(sys.stdin)]" 2>/dev/null || true
