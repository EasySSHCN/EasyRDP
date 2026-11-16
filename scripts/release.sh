#!/usr/bin/env bash
# ============================================================================
# EasyRDP 一键发布脚本（正式流程，借鉴 EasySSH scripts/release.sh 整理改进）
# ----------------------------------------------------------------------------
# 流程：凭据解密(加密库) → 构建与资产打包(可跳) → SHA256 校验 → git 提交/打 tag/
#       双平台 push → GitHub Release 同步发布 → Gitee Release 同步发布 → 汇总
#
# 用法（WSL bash 或任意 Linux bash）：
#   bash scripts/release.sh [版本号] [发布说明(可选,默认取 README 最新版本条目)]
#   bash scripts/release.sh 0.1.0 "修复若干"        # 附言模式
#   bash scripts/release.sh 0.1.0 --skip-build      # 跳过构建（资产已就绪）
#
# 凭据（优先级）：环境变量 GITHUB_TOKEN/GITEE_TOKEN → 项目加密库 secrets-vault/
#   （creds.enc + key，AES-256-CBC/PBKDF2；明文 token 仅存在于临时变量，不落盘）
#
# 与 EasySSH 流程的差异（EasyRDP 特化）：
#   - 构建是 Windows 端 Rust/Tauri（经 powershell.exe 调 package-release.ps1 +
#     make-release-assets.ps1），非 node/electron
#   - 附件五件套：portable.exe / win32-x64-portable.zip / Gateway-win-x64.zip /
#     src tar.gz / SHA256SUMS.txt（源码 tar 也作为资产发布）
#   - GitHub push 用 x-access-token 用户 + 强制 HTTP/1.1（WSL git HTTP/2 网络层
#     实测会 curl 16 framing 错误；EasySSHCN 用户名形式实测认证失败）
#   - Gitee 同 tag Release：默认删除重建（资产五件全量替换，幂等）
# ============================================================================
set -uo pipefail

# ---------- 参数与环境 ----------
VER="${1:-0.1.0}"
NOTE_ARG=""
SKIP_BUILD=0
case "${2:-}" in
  --skip-build) SKIP_BUILD=1 ;;
  "") ;;
  *) NOTE_ARG="$2" ;;
esac
TAG="v$VER"

# 项目根（本脚本位于 <根>/scripts/ 下；WSL 与 Linux 双兼容）
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [ -d /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP ]; then
  # WSL 场景强制对齐 B: 盘工作区（避免在 /tmp 解包树里误跑）
  ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
fi
DIST="$ROOT/easyrdp-client/dist"
VAULT="$ROOT/secrets-vault"

echo "==> 1/7 凭据解密（secrets-vault 加密库）"
GH_TOKEN="${GITHUB_TOKEN:-}"
GE_TOKEN="${GITEE_TOKEN:-}"
if [ -f "$VAULT/creds.enc" ] && [ -f "$VAULT/key" ]; then
  PLAIN=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/creds.enc" -pass file:"$VAULT/key" 2>/dev/null) \
    && echo "  加密库解密成功" || echo "  ⚠ 加密库解密失败（key/creds.enc 不匹配？）"
  [ -z "$GH_TOKEN" ] && GH_TOKEN=$(printf '%s\n' "$PLAIN" | sed -n 's/^github_api_token: //p')
  [ -z "$GE_TOKEN" ] && GE_TOKEN=$(printf '%s\n' "$PLAIN" | sed -n 's/^gitee_api_token: //p')
fi
[ -n "$GH_TOKEN" ] && echo "  GitHub token: 已就绪(EasySSHCN)" || echo "  ⚠ GitHub token 缺失——将跳过 GitHub 步骤"
[ -n "$GE_TOKEN" ] && echo "  Gitee  token: 已就绪(easyssh)"   || echo "  ⚠ Gitee token 缺失——将跳过 Gitee 步骤"

# ---------- 构建与资产 ----------
echo "==> 2/7 构建 + 资产打包"
if [ "$SKIP_BUILD" -eq 0 ]; then
  # 修复：cygpath 在 WSL 不存在，必须用 wslpath -w 转换为 Windows 形式路径；
  # 构建失败必须硬终止（此前 || echo 会带着缺失资产继续走，靠第 3 步兜底太晚）
  PS_PKG=$(wslpath -w "$ROOT/easyrdp-client/scripts/package-release.ps1") || exit 1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PS_PKG" \
    || { echo "  ✗ 构建失败，终止"; exit 1; }
  echo "  构建 OK（cargo release + dist\EasyRDP-win64.zip）"
  PS_AST=$(wslpath -w "$ROOT/make-release-assets.ps1") || exit 1
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PS_AST" \
    || { echo "  ✗ 资产打包失败，终止"; exit 1; }
  echo "  资产打包 OK（portable.zip / src tar.gz / SHA256SUMS）"
else
  echo "  跳过构建（--skip-build）"
fi

SRC="$ROOT/EasyRDP-src-$VER.tar.gz"
EXE="$DIST/EasyRDP-$VER-portable.exe"
ZIP="$DIST/EasyRDP-$VER-win32-x64-portable.zip"
GW="$ROOT/dist-gateway/EasyRDP-Gateway-$VER-win-x64.zip"
SUMS="$DIST/SHA256SUMS.txt"

echo "==> 3/7 资产完整性校验"
MISS=0
for f in "$EXE" "$ZIP" "$GW" "$SRC" "$SUMS"; do
  [ -f "$f" ] && echo "  ✓ $(basename "$f")  $(du -h "$f" | cut -f1)" || { echo "  ✗ 缺失 $(basename "$f")"; MISS=1; }
done
[ "$MISS" -eq 0 ] || { echo "资产缺失，终止（先跑 make-release-assets.ps1）"; exit 1; }
# SHA256SUMS 与实际文件比对（portable.exe 为权威样本）
WANT=$(grep "EasyRDP-$VER-portable.exe" "$SUMS" | awk '{print $1}')
HAVE=$(sha256sum "$EXE" | awk '{print $1}')
if [ "$WANT" = "$HAVE" ]; then echo "  ✓ SHA256SUMS 与 portable.exe 一致"; else echo "  ⚠ SHA256SUMS 不一致（先重跑 make-release-assets.ps1）"; fi

# —— 脱敏门禁（📌 永久规则：对外（官网、GitHub、Gitee等）发布源码包时候必须脱敏）——
echo "==> 3.5/8 源码包脱敏门禁"
LEAK=$(tar -tzf "$SRC" | grep -E 'secrets-vault|signing/|\.enc$|id_ed25519|\.ghtoken|\.getoken' | head -3)
if [ -n "$LEAK" ]; then
  echo "  ✗ 脱敏门禁失败，发布终止。命中："
  echo "$LEAK"
  exit 1
fi
echo "  ✓ 无 secrets-vault/signing/密钥/凭据文件泄漏"

# ---------- git 提交 + tag + 双平台 push ----------
echo "==> 4/7 git 提交 + tag + push（GitHub/Gitee 镜像）"
RELDIR=$(mktemp -d)
tar -xzf "$SRC" -C "$RELDIR" --strip-components=1
cd "$RELDIR"
git init -q -b main
git add -A
git -c user.name=EasySSHCN -c user.email=ad@6xm.net commit -q -m "release: $TAG" || echo "  (无变更可提交)"
git tag -f "$TAG"
if [ -n "$GH_TOKEN" ]; then
  # 关键：x-access-token 用户 + HTTP/1.1（见文件头「与 EasySSH 的差异」）
  git -c http.version=HTTP/1.1 push -q -f \
    "https://x-access-token:${GH_TOKEN}@github.com/EasySSHCN/EasyRDP.git" main --tags \
    && echo "  GitHub push OK" || echo "  ⚠ GitHub push 失败"
else
  echo "  跳过 GitHub push（无 token）"
fi
if [ -n "$GE_TOKEN" ]; then
  git -c http.version=HTTP/1.1 push -q -f \
    "https://easyssh:${GE_TOKEN}@gitee.com/easyssh/EasyRDP.git" main --tags \
    && echo "  Gitee push OK" || echo "  ⚠ Gitee push 失败"
  curl -s -X PATCH "https://gitee.com/api/v5/repos/easyssh/EasyRDP" \
    -d "access_token=$GE_TOKEN" -d "private=false" | grep -o '"private":false' | head -1 >/dev/null \
    && echo "  Gitee 仓库已确认为公开" || true
else
  echo "  跳过 Gitee push（无 token）"
fi

# ---------- Release 说明（README 权威源 + 可选附言） ----------
echo "==> 5/7 Release 说明（取 README「### v$VER」条目）"
NOTES=$(awk -v ver="### v$VER" 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$ROOT/README.md")
[ -n "${NOTES// }" ] || NOTES="EasyRDP $VER"
[ -n "$NOTE_ARG" ] && NOTES="$NOTE_ARG

$NOTES"
BODY="## EasyRDP $TAG

**官方网站（下载 / 文档）：https://EasyRDP.CN**

### 本版更新

$NOTES"
BODY_JSON=$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")
ASSETS=("EasyRDP-$VER-portable.exe:$EXE" "EasyRDP-$VER-win32-x64-portable.zip:$ZIP" \
        "EasyRDP-Gateway-$VER-win-x64.zip:$GW" "EasyRDP-src-$VER.tar.gz:$SRC" "SHA256SUMS.txt:$SUMS")

# ---------- GitHub Release ----------
echo "==> 6/7 GitHub Release 同步发布"
if [ -n "$GH_TOKEN" ]; then
  REL_ID=$(curl -s -H "Authorization: token $GH_TOKEN" \
    "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/tags/$TAG" \
    | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
  if [ -n "$REL_ID" ]; then
    echo "  Release 已存在 id=$REL_ID——替换全部附件"
    for A in $(curl -s -H "Authorization: token $GH_TOKEN" \
        "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/$REL_ID/assets" \
        | python3 -c "
import json,sys
for a in json.load(sys.stdin): print(a['id'])"); do
      curl -s -X DELETE -H "Authorization: token $GH_TOKEN" \
        "https://api.github.com/repos/EasySSHCN/EasyRDP/releases/assets/$A" -o /dev/null
    done
  else
    REL=$(mktemp)
    curl -s -X POST -H "Authorization: token $GH_TOKEN" -H "Accept: application/vnd.github+json" \
      "https://api.github.com/repos/EasySSHCN/EasyRDP/releases" \
      -d "{\"tag_name\":\"$TAG\",\"name\":\"EasyRDP $TAG\",\"body\":$BODY_JSON,\"prerelease\":false}" > "$REL"
    REL_ID=$(python3 -c "import json;print(json.load(open('$REL')).get('id',''))")
    [ -n "$REL_ID" ] && echo "  Release 创建成功 id=$REL_ID" || { echo "  ⚠ Release 创建失败:"; cat "$REL"; }
    rm -f "$REL"
  fi
  for pair in "${ASSETS[@]}"; do
    N="${pair%%:*}"; F="${pair#*:}"
    curl -s -X POST -H "Authorization: token $GH_TOKEN" -H "Content-Type: application/octet-stream" \
      --data-binary @"$F" \
      "https://uploads.github.com/repos/EasySSHCN/EasyRDP/releases/$REL_ID/assets?name=$N" \
      | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GH',d.get('name'),'=',d.get('state') or d.get('message'))"
  done
  echo "  https://github.com/EasySSHCN/EasyRDP/releases/tag/$TAG"
else
  echo "  跳过（无 GitHub token）"
fi

# ---------- Gitee Release ----------
echo "==> 7/7 Gitee Release 同步发布"
if [ -n "$GE_TOKEN" ]; then
  # 同 tag Release 删除重建（五件资产全量替换；token 必须在查询串——表单字段会挂起）
  OLD_GID=$(curl -s "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/tags/$TAG?access_token=$GE_TOKEN" \
    | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
  [ -n "$OLD_GID" ] && curl -s -X DELETE \
    "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$OLD_GID?access_token=$GE_TOKEN" \
    -o /dev/null -w "  旧 Release 已删除 http %{http_code}\n"
  GREL=$(mktemp)
  curl -s -X POST -H "Content-Type: application/json" "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases" \
    -d "{\"access_token\":\"$GE_TOKEN\",\"tag_name\":\"$TAG\",\"target_commitish\":\"main\",\"name\":\"EasyRDP $TAG\",\"body\":$BODY_JSON,\"prerelease\":false}" > "$GREL"
  GID=$(python3 -c "import json;d=json.load(open('$GREL'));print(d.get('id','') or d.get('original_id','') if isinstance(d,dict) else '')")
  rm -f "$GREL"
  echo "  新 Release id=$GID"
  for pair in "${ASSETS[@]}"; do
    F="${pair#*:}"
    SZ=$(stat -c%s "$F" 2>/dev/null || echo 0)
    if [ "$SZ" -gt 104857600 ]; then
      echo "  ⏭ 跳过 $(basename "$F")（超 Gitee 100MB 附件上限）"
      continue
    fi
    curl -s --max-time 7200 -X POST \
      "https://gitee.com/api/v5/repos/easyssh/EasyRDP/releases/$GID/attach_files?access_token=$GE_TOKEN" \
      -F "file=@$F" \
      | python3 -c "import json,sys;d=json.load(sys.stdin);print('  GE',d.get('name') or d.get('message'))" | head -1
  done
  echo "  https://gitee.com/easyssh/EasyRDP/releases/tag/$TAG"
else
  echo "  跳过（无 Gitee token）"
fi

rm -rf "$RELDIR"

# ---------- 官网同步（第 8 步：easyrdp.cn）+ 逐条校验（8.5，全过才算发布完成）----------
echo "==> 8/8 官网同步（easyrdp.cn）"
SERVER_ENC="$ROOT/secrets-vault/site-server.enc"
if [ -f "$SERVER_ENC" ] && [ -f "$ROOT/secrets-vault/key" ]; then
  SITE_HOST=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
  SITE_PORT=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
  # 修复：enc 文件 webroot 行带注释（如 "/www/wwwroot/easyrdp.cn (nginx)"），只取第一个 token
  SITE_DIR=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SERVER_ENC" -pass file:"$ROOT/secrets-vault/key" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
  # 修复：WSL 直用 /mnt/p 密钥会 0777 被拒 → 拷贝 /tmp chmod 600，用后删除
  KEY_PATH=/tmp/id_ed25519_easyssh
  cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH" \
    || { echo "  ✗ SSH 密钥准备失败"; exit 1; }
  SITE_OK=0
  if [ -n "$SITE_HOST" ] && [ -n "$SITE_DIR" ]; then
    SITE="$ROOT/easyrdp-site"

    # —— 官网版本徽章同步 + 全页版本串替换（VERBADGE-20261116 流程节点）——
    OLD_VER=$(sed 's/\r$//' "$SITE/assets/js/main.js" | sed -n 's/^\s*var SITE_VERSION = "\(.*\)";\r*$/\1/p' | head -1)
    echo "  版本徽章: ${OLD_VER:-未知} → v$VER"
    # 全页版本串替换（除 changelog.html：其条目为人工撰写的权威变更记录，
    # 由随后的 8a 前置校验把关；其余页面所有旧版本串统一替换为本次版本，
    # alpha 历史版本名保护不误伤）
    OLD_NUM="${OLD_VER#v}"
    if [ -n "$OLD_NUM" ] && [ "$OLD_NUM" != "$VER" ]; then
      for f in "$SITE"/*.html; do
        [ "$(basename "$f")" = "changelog.html" ] && continue
        sed -i "s/$OLD_NUM-alpha/__ALPHA_GUARD__/g; s/$OLD_NUM/$VER/g; s/__ALPHA_GUARD__/$OLD_NUM-alpha/g" "$f"
      done
      echo "  全页版本串替换: $OLD_NUM → $VER（changelog.html 除外）"
    fi
    if [ "$OLD_VER" != "v$VER" ]; then
      sed -i 's|^\(\s*\)var SITE_VERSION = ".*";.*$|\1var SITE_VERSION = "v'"$VER"'";|' "$SITE/assets/js/main.js"
    fi

    # —— 8a 发布前置校验（上传前）：版本文案未更新 = 发布内容不完整，直接终止 ——
    echo "  -- 8a 版本文案前置校验 --"
    PRE_FAIL=0
    grep -q "<h2>v$VER</h2>" "$SITE/changelog.html" \
      || { echo "  ✗ changelog.html 缺少 v$VER 条目——发布前必须先人工撰写本版变更记录"; PRE_FAIL=1; }
    grep -A3 "<h2>v$VER</h2>" "$SITE/changelog.html" | grep -q "当前构建" \
      || { echo "  ✗ changelog.html 「当前构建」徽章未迁移到 v$VER 块"; PRE_FAIL=1; }
    grep -q "releases/tag/v$VER" "$SITE/download.html" \
      || { echo "  ✗ download.html GitHub Release 链接未指向 v$VER"; PRE_FAIL=1; }
    grep -q "下载 EasyRDP v$VER" "$SITE/download.html" \
      || { echo "  ✗ download.html meta 描述未更新为 v$VER"; PRE_FAIL=1; }
    LEFT=$(grep -rl "$OLD_NUM" "$SITE" --include='*.html' 2>/dev/null | grep -v changelog || true)
    if [ -n "$LEFT" ] && [ -n "$OLD_NUM" ] && [ "$OLD_NUM" != "$VER" ]; then
      echo "  ⚠ 以下页面仍有旧版本串 $OLD_NUM（历史性表述请改写，避免误导）: $(echo "$LEFT" | xargs -n1 basename | tr '\n' ' ')"
    fi
    [ "$PRE_FAIL" -eq 1 ] && { echo "  ✗ 版本文案前置校验失败——流程终止（先更新 changelog.html / download.html 再重跑）"; rm -f "$KEY_PATH"; exit 1; }
    echo "  ✓ 版本文案前置校验通过（changelog 条目/当前构建徽章/下载页链接与描述）"
    CUR_V=$(ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" \
      "grep -o 'main.js?v=[0-9]*' $SITE_DIR/index.html 2>/dev/null | head -1 | grep -o '[0-9]*\$' || echo 0")
    NEW_V=$(( ${CUR_V:-0} + 1 ))
    for f in "$SITE"/*.html; do
      sed -i -E "s|assets/js/main\.js(\?v=[0-9]*)?|assets/js/main.js?v=$NEW_V|g" "$f"
    done
    echo "  main.js 缓存版本: ?v=${CUR_V:-0} → ?v=$NEW_V"
    scp -q -o StrictHostKeyChecking=accept-new -i "$KEY_PATH" -P "${SITE_PORT:-22}" \
      "$SITE"/*.html "$SITE_HOST:$SITE_DIR/" || echo "  ⚠ 页面上传失败"
    scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" \
      "$SITE/assets/js/main.js" "$SITE_HOST:$SITE_DIR/assets/js/main.js" || echo "  ⚠ main.js 上传失败"
    ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "mkdir -p $SITE_DIR/downloads"
    for pair in "EasyRDP-$VER-portable.exe:$EXE" "EasyRDP-$VER-win32-x64-portable.zip:$ZIP" \
                "EasyRDP-Gateway-$VER-win-x64.zip:$GW" "EasyRDP-src-$VER.tar.gz:$SRC" "SHA256SUMS.txt:$SUMS"; do
      N="${pair%%:*}"; F="${pair#*:}"
      scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$F" "$SITE_HOST:$SITE_DIR/downloads/$N" \
        && echo "  ✓ downloads/$N" || echo "  ⚠ downloads/$N 上传失败"
    done

    # —— 8b 线上逐条校验：任何一条失败 = 发布流程未完成（exit 1）——
    echo "  -- 8b 线上逐条校验 --"
    sleep 1
    PASS=0; FAIL=0
    ck() {  # ck <描述> <条件命令...>
      local desc="$1"; shift
      if "$@" >/dev/null 2>&1; then echo "  ✓ $desc"; PASS=$((PASS+1))
      else echo "  ✗ $desc"; FAIL=$((FAIL+1)); fi
    }
    LIVE=$(curl -s "https://easyrdp.cn/assets/js/main.js?v=$NEW_V&nocache=$RANDOM")
    [ "$(echo "$LIVE" | sed -n 's/^\s*var SITE_VERSION = "\(.*\)";\r*$/\1/p' | head -1)" = "v$VER" ] \
      && ck "① 徽章常量 = v$VER" true || ck "① 徽章常量 = v$VER（线上: $(echo "$LIVE" | grep -o 'SITE_VERSION = "[^"]*"' | head -1)）" false
    CH=$(curl -s "https://easyrdp.cn/changelog.html?nocache=$RANDOM")
    echo "$CH" | grep -q "<h2>v$VER</h2>" \
      && ck "② changelog 含 v$VER 条目" true || ck "② changelog 含 v$VER 条目" false
    echo "$CH" | grep -A3 "<h2>v$VER</h2>" | grep -q "当前构建" \
      && ck "③ 「当前构建」徽章在 v$VER 块" true || ck "③ 「当前构建」徽章在 v$VER 块" false
    if echo "$CH" | grep -A6 "<h2>v0.1.0（正式版）</h2>" | grep -q "当前构建"; then ck "④ 旧版本块已摘除「当前构建」" false
    else ck "④ 旧版本块已摘除「当前构建」" true; fi
    DL=$(curl -s "https://easyrdp.cn/download.html?nocache=$RANDOM")
    echo "$DL" | grep -q "releases/tag/v$VER" \
      && ck "⑤ download.html Release 链接指向 v$VER" true || ck "⑤ download.html Release 链接指向 v$VER" false
    echo "$DL" | grep -q "下载 EasyRDP v$VER" \
      && ck "⑥ download.html 描述已更新 v$VER" true || ck "⑥ download.html 描述已更新 v$VER" false
    IX=$(curl -s "https://easyrdp.cn/index.html?nocache=$RANDOM")
    echo "$IX" | grep -q "assets/js/main.js?v=$NEW_V" \
      && ck "⑦ 首页引用最新 main.js?v=$NEW_V" true || ck "⑦ 首页引用最新 main.js?v=$NEW_V" false
    for pair in "EasyRDP-$VER-portable.exe" "EasyRDP-$VER-win32-x64-portable.zip" \
                "EasyRDP-Gateway-$VER-win-x64.zip" "EasyRDP-src-$VER.tar.gz" "SHA256SUMS.txt"; do
      CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/downloads/$pair")
      [ "$CODE" = "200" ] && ck "⑧ downloads/$pair 可下载(200)" true || ck "⑧ downloads/$pair HTTP $CODE" false
    done
    echo "  校验汇总: 通过 $PASS / 失败 $FAIL"
    rm -f "$KEY_PATH"
    if [ "$FAIL" -eq 0 ]; then
      SITE_OK=1
      echo "  ✅ 官网逐条校验全部通过"
      echo "  官网已同步: https://easyrdp.cn"
    else
      echo "  ✗ 官网校验存在失败项——发布流程【未完成】，请排查后重跑 release.sh（--skip-build 可复用资产）"
      exit 1
    fi
  else
    echo "  ⚠ 加密库解析失败，跳过官网同步"
    rm -f "$KEY_PATH"
    exit 1
  fi
else
  echo "  ⚠ 无 site-server.enc，跳过官网同步"
  exit 1
fi

echo "✅ EasyRDP $TAG 发布流程结束（官网逐条校验全过）"
