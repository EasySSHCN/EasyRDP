#!/usr/bin/env bash
# EasyRDP 首发脚本 —— 参考 EasySSH release.sh / repo-sync.sh 流程（v0.1.0-alpha 源码发布）
# 用法: bash /tmp/easyrdp-release.sh <源码tar.gz路径>
set -euo pipefail
VER=0.1.0-alpha
TAG="v$VER"
APP=/opt/dsh/workspace/EasyRDP
SRC="${1:?用法: easyrdp-release.sh <tar.gz路径>}"
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
GH_REPO="EasySSHCN/EasyRDP"
GE_REPO="easyssh/EasyRDP"

echo "==> [1/6] 解压源码"
rm -rf "$APP"; mkdir -p "$APP"
tar -xzf "$SRC" -C "$APP" --strip-components=1
ls "$APP"

echo "==> [2/6] git init + commit + tag"
cd "$APP"
git init -q -b main
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "release: $TAG 首次发布（客户端骨架 + 网关骨架 + 官网 + PRD）"
git tag -f "$TAG"
git log --oneline | head -2

echo "==> [3/6] push GitHub"
git push -q -f "https://EasySSHCN:${TOKEN}@github.com/${GH_REPO}.git" main --tags && echo "GitHub push OK"

echo "==> [4/6] push Gitee（并确认仓库可见性）"
git push -q -f "https://easyssh:${GITEE}@gitee.com/${GE_REPO}.git" main --tags && echo "Gitee push OK"
curl -s -X PATCH "https://gitee.com/api/v5/repos/${GE_REPO}" \
  -d "access_token=$GITEE" -d "private=false" | grep -o '"private":false' | head -1 || true

echo "==> [5/6] Release 说明"
NOTES=$(awk -v ver="### $TAG " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' README.md)
BODY="## EasyRDP $TAG

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新
$NOTES"

echo "==> [6/6] 创建 Release"
SHA=$(mktemp); sha256sum "$SRC" | awk '{print $1"  "$2}' > /tmp/EasyRDP-src-SHA256SUMS.txt
# GitHub Release
EX=$(curl -s "https://api.github.com/repos/$GH_REPO/releases/tags/$TAG" | grep -o '"id": *[0-9]*' | head -1 || true)
if [ -z "$EX" ]; then
  REL=$(mktemp)
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/$GH_REPO/releases" \
    -d "{\"tag_name\":\"$TAG\",\"name\":\"EasyRDP $TAG\",\"body\":$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY"),\"prerelease\":true}" > "$REL"
  REL_ID=$(python3 -c "import json;print(json.load(open('$REL')).get('id',''))")
  [ -n "$REL_ID" ] && echo "GitHub Release 创建成功 id=$REL_ID" || { echo "GitHub Release 失败:"; cat "$REL"; }
  rm -f "$REL"
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @"$SRC" \
    "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=EasyRDP-$VER-source.tar.gz" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GitHub 附件',d.get('name'),'=',d.get('state') or d.get('message'))"
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @/tmp/EasyRDP-src-SHA256SUMS.txt \
    "https://uploads.github.com/repos/$GH_REPO/releases/$REL_ID/assets?name=SHA256SUMS.txt" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GitHub 附件',d.get('name'),'=',d.get('state') or d.get('message'))"
else
  echo "GitHub Release 已存在,跳过"
fi
# Gitee Release
GID=$(curl -s "https://gitee.com/api/v5/repos/$GE_REPO/releases/tags/$TAG?access_token=$GITEE" | python3 -c "
import json,sys
try:
  d=json.load(sys.stdin)
  print(d.get('id','') if isinstance(d,dict) else '')
except Exception:
  print('')" || true)
if [ -z "$GID" ]; then
  GREL=$(mktemp)
  curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases" \
    -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"$TAG\",\"target_commitish\":\"main\",\"name\":\"EasyRDP $TAG\",\"body\":$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY"),\"prerelease\":true}" > "$GREL"
  GID=$(python3 -c "import json;d=json.load(open('$GREL'));print(d.get('id','') or d.get('original_id','') if isinstance(d,dict) else '')")
  [ -n "$GID" ] && echo "Gitee Release 创建成功 id=$GID" || { echo "Gitee Release 失败:"; cat "$GREL"; }
  rm -f "$GREL"
  if [ -n "$GID" ]; then
    curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
      -F "file=@$SRC" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  Gitee 附件',d.get('name') or d.get('message'))" | head -1
    curl -s -X POST "https://gitee.com/api/v5/repos/$GE_REPO/releases/$GID/attach_files?access_token=$GITEE" \
      -F "file=@/tmp/EasyRDP-src-SHA256SUMS.txt" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  Gitee 附件',d.get('name') or d.get('message'))" | head -1
  fi
else
  echo "Gitee Release 已存在,跳过"
fi
echo "✔ EasyRDP $TAG 发布完成"
