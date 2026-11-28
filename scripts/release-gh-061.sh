#!/usr/bin/env bash
# release-gh-061.sh — v0.6.1 源码推送 + GitHub/Gitee Release 创建与附件
set -e
VT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
D=$R/easyrdp-client/dist
SRC=$R/EasyRDP-src-0.6.1.tar.gz
TAG=v0.6.1
PLAIN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VT/creds.enc" -pass file:"$VT/key")
GH=$(printf '%s\n' "$PLAIN" | sed -n 's/^github_api_token: //p')
GE=$(printf '%s\n' "$PLAIN" | sed -n 's/^gitee_api_token: //p')
BODY="窗口几何记忆修复：拖动边缘改变大小 -> 关闭 -> 再开，按关闭时的大小/位置/最大化状态打开；断开会话不再重置窗口尺寸。

Assets:
- EasyRDP-0.6.1-portable.exe
- EasyRDP-0.6.1-win32-x64-portable.zip
- EasyRDP-Gateway-0.6.1-win-x64.zip
- EasyRDP-src-0.6.1.tar.gz
- SHA256SUMS.txt"

echo "== 1) 解包源码并推送 GitHub main + tag =="
RELDIR=$(mktemp -d)
tar -xzf "$SRC" -C "$RELDIR" --strip-components=1
cd "$RELDIR"
git init -q -b main 2>/dev/null || true
git add -A
git -c user.name=EasySSHCN -c user.email=release@easyrdp.cn commit -qm "release: $TAG"
git tag -f "$TAG"
PUSHED=0
for i in 1 2 3; do
  if git -c http.version=HTTP/1.1 push -q -f "https://x-access-token:${GH}@github.com/EasySSHCN/EasyRDP.git" main --tags; then
    echo "GitHub push OK (attempt $i)"; PUSHED=1; break
  fi
  echo "  attempt $i failed, retry..."; sleep 5
done
[ "$PUSHED" = 1 ] || { echo "GitHub push failed"; exit 1; }

echo "== 2) GitHub Release 创建 + 附件 =="
RID=$(curl -s -X POST -H "Authorization: Bearer $GH" \
  -H "Accept: application/vnd.github+json" \
  "https://api.github.com/repos/EasySSHCN/EasyRDP/releases" \
  -d "$(printf '{"tag_name":"%s","name":"v0.6.1 窗口几何记忆修复","body":"%s","draft":false,"prerelease":false}' "$TAG" "$(printf '%s' "$BODY" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read())[1:-1])')")" \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d.get("id","ERR"))')
echo "GitHub release id=$RID"
for F in "$D/EasyRDP-0.6.1-portable.exe" "$D/EasyRDP-0.6.1-win32-x64-portable.zip" "$R/dist-gateway/EasyRDP-Gateway-0.6.1-win-x64.zip" "$SRC" "$D/SHA256SUMS.txt"; do
  N=$(basename "$F")
  CODE=$(curl -s -o /tmp/gh-up.json -w "%{http_code}" --max-time 3600 \
    -H "Authorization: Bearer $GH" -H "Content-Type: application/octet-stream" \
    --data-binary "@$F" "https://uploads.github.com/repos/EasySSHCN/EasyRDP/releases/$RID/assets?name=$N")
  echo "GH $N -> HTTP $CODE"
done

echo "== 3) Gitee Release 创建 + 附件 =="
GBODY=$(printf '%s' "$BODY" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read()))')
RID2=$(curl -s -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases?access_token=$GE" \
  -H "Content-Type: application/json" \
  -d "{\"tag_name\":\"$TAG\",\"name\":\"v0.6.1 窗口几何记忆修复\",\"body\":$GBODY,\"target_commitish\":\"main\",\"prerelease\":false}" \
  | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d.get("id","ERR"))')
echo "Gitee release id=$RID2"
for F in "$D/EasyRDP-0.6.1-portable.exe" "$D/EasyRDP-0.6.1-win32-x64-portable.zip" "$R/dist-gateway/EasyRDP-Gateway-0.6.1-win-x64.zip" "$SRC" "$D/SHA256SUMS.txt"; do
  N=$(basename "$F")
  CODE=$(curl -s -o /tmp/ge-up.json -w "%{http_code}" --max-time 3600 \
    -X POST "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$RID2/attach_files?access_token=$GE" \
    -F "file=@$F")
  echo "GE $N -> HTTP $CODE"
done
rm -rf "$RELDIR"
echo "== 完成 =="
