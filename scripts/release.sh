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

# —— 网关配置模板策略矩阵校验（POLICY-GATE 20261121：F5 deny by default 生产 403 事故固化）——
# 生产部署模板即 easyrdp-gateway/config.example.toml（README: cp config.example.toml config.toml；
# Dockerfile 同名拷贝；仓库内无 config.toml.example）。F5 策略矩阵 deny by default：
# 配置缺 [policy] 段或无任何 [[policy.rules]] 时所有 user 角色 Token 隧道 403。
echo "==> 3.6/8 网关配置模板策略矩阵校验"
GW_TPL="$ROOT/easyrdp-gateway/config.example.toml"
POL_FAIL=0
[ -f "$GW_TPL" ] || { echo "  ✗ 网关策略矩阵规则缺失（deny by default 会导致隧道 403）: 缺少模板 $GW_TPL"; POL_FAIL=1; }
if [ -f "$GW_TPL" ]; then
  grep -q '^[[:space:]]*\[policy\]' "$GW_TPL" \
    || { echo "  ✗ 网关策略矩阵规则缺失（deny by default 会导致隧道 403）: 模板缺 [policy] 段"; POL_FAIL=1; }
  grep -q '^[[:space:]]*\[\[policy\.rules\]\]' "$GW_TPL" \
    || { echo "  ✗ 网关策略矩阵规则缺失（deny by default 会导致隧道 403）: 模板缺 [[policy.rules]] 条目"; POL_FAIL=1; }
fi
[ "$POL_FAIL" -eq 0 ] || { echo "  ✗ 策略矩阵校验失败，发布终止"; exit 1; }
echo "  ✓ 网关配置模板含 [policy] 段与 [[policy.rules]] 规则"

# —— 发布脚本 BOM 门禁（BOM-GATE 20261126：PS5.1 读无 BOM 中文脚本按 ANSI 解码导致
#    解析错乱——build-v4-local / make-release-assets 两次实证，资产打包曾因此拿错候选终止）——
echo "==> 3.7/8 发布脚本 BOM 校验"
BOM_OUT=$(powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$ROOT/scripts/fix-ps1-bom.ps1")" -Root "$(wslpath -w "$ROOT")" 2>/dev/null || true)
if printf '%s' "$BOM_OUT" | grep -qE '需修复: ?0|needfix=0'; then
  echo "  ✓ 全仓含中文 .ps1 均带 UTF-8 BOM"
else
  echo "$BOM_OUT"
  # 自动修复后复核一次，仍不过才终止
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$ROOT/scripts/fix-ps1-bom.ps1")" -Root "$(wslpath -w "$ROOT")" -Fix >/dev/null 2>&1 || true
  BOM_OUT=$(powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$ROOT/scripts/fix-ps1-bom.ps1")" -Root "$(wslpath -w "$ROOT")" 2>/dev/null || true)
  if printf '%s' "$BOM_OUT" | grep -qE '需修复: ?0|needfix=0'; then
    echo "  ✓ 已自动补齐缺 BOM 的 .ps1（自动修复后复核通过）"
  else
    echo "  ✗ 仍有 .ps1 缺 BOM，发布终止（手工运行 scripts/fix-ps1-bom.ps1 -Fix 后重试）"
    exit 1
  fi
fi

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
# CWD-FIX 20261128：RELDIR 已删除，残留的已删除 cwd 会让后续 wslpath/子进程全部
# 报 "No such file or directory"（v0.6.2 发布三轮实证）——立即回 ROOT 再进第 8 步。
cd "$ROOT" || exit 1

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

    # —— AUTO-SITE 20261128：官网随版内容自动化（changelog 条目 + 徽章迁移 +
    #    download 描述 + 官网更新记录 + manual 随版检测；8a 的前置，与 8a 同级把关）——
    echo "  -- 8.0 prepare-site 官网内容自动化 --"
    if ! bash "$ROOT/scripts/prepare-site.sh" "$VER"; then
      echo "  ✗ prepare-site 失败（manual 待更新清单见上）——发布会话按清单补页/改文案后重跑 release.sh（--skip-build 可复用资产）"
      rm -f "$KEY_PATH"; exit 1
    fi

    # —— 官网版本徽章同步 + 全页版本串替换（VERBADGE-20261116 流程节点）——
    OLD_VER=$(sed 's/\r$//' "$SITE/assets/js/main.js" | sed -n 's/^\s*var SITE_VERSION = "\(.*\)";\r*$/\1/p' | head -1)
    echo "  版本徽章: ${OLD_VER:-未知} → v$VER"
    # 全页版本串替换（除 changelog.html：其条目为人工撰写的权威变更记录，
    # 由随后的 8a 前置校验把关；其余页面所有旧版本串统一替换为本次版本，
    # alpha 历史版本名保护不误伤）
    OLD_NUM="${OLD_VER#v}"
    if [ -n "$OLD_NUM" ] && [ "$OLD_NUM" != "$VER" ]; then
      for f in "$SITE"/*.html "$SITE"/en/*.html; do
        [ "$(basename "$f")" = "changelog.html" ] && continue
        sed -i "s/$OLD_NUM-alpha/__ALPHA_GUARD__/g; s/$OLD_NUM/$VER/g; s/__ALPHA_GUARD__/$OLD_NUM-alpha/g" "$f"
      done
      echo "  全页版本串替换: $OLD_NUM → $VER（changelog.html 除外，含 en/）"
    fi
    # R-20261118-006 流程固化：历史版本表/支持版本表/EN 版本串/JSON-LD 自动收编
    # （sed 顶替会把人工策划的两张表顶坏——download 表次行被顶成新版本、security 表错位——
    #   由 curate_site.py 按 dist 实测尺寸重建，永远最近 3 条降序。幂等可重跑。）
    if python3 "$ROOT/scripts/curate_site.py" --ver "$VER" --date "$(date +%F)" \
         --old "${OLD_NUM:-$VER}" --site "$SITE" --dist "$DIST" --gw "$ROOT/dist-gateway"; then
      echo "  表格/支持版本/EN 版本串自动收编完成"
    else
      echo "  ✗ 表格收编失败，终止（手工修复后重跑）"; rm -f "$KEY_PATH"; exit 1
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

    # —— 子目录同步（SUBDIR-DEPLOY 20261120 固化：v0.4.0 manual/ 漏部署实证）——
    # 覆盖式上传（增量，不删除服务器独有文件如 api/*/keys.php）；上传后统一 chown www:www。
    ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "mkdir -p $SITE_DIR/en $SITE_DIR/manual $SITE_DIR/api $SITE_DIR/assets/data"
    scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$SITE"/en/*.html "$SITE_HOST:$SITE_DIR/en/" \
      && echo "  ✓ en/ 页面同步" || echo "  ⚠ en/ 上传失败"
    scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$SITE"/manual/*.html "$SITE_HOST:$SITE_DIR/manual/" \
      && echo "  ✓ manual/ 手册页同步" || echo "  ⚠ manual/ 上传失败"
    scp -q -r -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$SITE"/api/* "$SITE_HOST:$SITE_DIR/api/" \
      && echo "  ✓ api/ PHP 递归同步（服务器独有 keys.php 不受影响）" || echo "  ⚠ api/ 上传失败"
    scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$SITE"/assets/data/*.json "$SITE_HOST:$SITE_DIR/assets/data/" \
      && echo "  ✓ assets/data/（site-updates.json）同步" || echo "  ⚠ assets/data/ 上传失败"
    ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" \
      "chown -R www:www $SITE_DIR/en $SITE_DIR/manual $SITE_DIR/api $SITE_DIR/assets/data $SITE_DIR/downloads 2>/dev/null; echo '  ✓ chown www:www 完成'"

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
    CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/manual/index.html")
    [ "$CODE" = "200" ] && ck "⑨ manual/ 手册页可达" true || ck "⑨ manual/ 手册页 HTTP $CODE" false
    CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/en/download.html")
    [ "$CODE" = "200" ] && ck "⑩ en/download.html 可达" true || ck "⑩ en/download.html HTTP $CODE" false
    LIVE_UPD_CK=$(curl -s "https://easyrdp.cn/assets/data/site-updates.json?nocache=$RANDOM" | sed -n 's/.*"current": *"\([^"]*\)".*/\1/p' | head -1)
    LOCAL_UPD_CK=$(sed -n 's/.*"current": *"\([^"]*\)".*/\1/p' "$SITE/assets/data/site-updates.json" 2>/dev/null | head -1)
    { [ -n "$LIVE_UPD_CK" ] && [ "$LIVE_UPD_CK" = "$LOCAL_UPD_CK" ]; } \
      && ck "⑪ 官网更新记录 current=$LIVE_UPD_CK 已同步" true || ck "⑪ 官网更新记录（线上 ${LIVE_UPD_CK:-缺失} ≠ 本地 ${LOCAL_UPD_CK:-缺失}）——先补「官网更新记录」节点再重跑" false
    # —— ⑫ 线上 404.html 清理残留复验（AUTO-404 20261128 固化）：本次 README 小节
    #    涉及打点/遥测/脚本清理类变更时，线上抓取 404.html grep 关键词（反引号代码名 +
    #    「」入口名）；命中残留即失败。无清理类变更 → 记 SKIP 不计入失败。——
    CLEAN_LINES=$(awk -v ver="### v$VER" 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$ROOT/README.md" \
      | grep -E '清理|残留|移除|下线|回收|删除.*打点|遥测|埋点|上报脚本' || true)
    if [ -n "$CLEAN_LINES" ]; then
      KW=$(printf '%s\n' "$CLEAN_LINES" | grep -oE '`[^`]+`' | tr -d '`' | grep -vE '^\s*$' || true)
      KW2=$(printf '%s\n' "$CLEAN_LINES" | grep -oE '「[^「」]+」' | tr -d '「」' || true)
      PAGE_404=$(curl -s "https://easyrdp.cn/404.html?nocache=$RANDOM")
      HITS=""
      for kw in $KW $KW2; do
        if printf '%s' "$PAGE_404" | grep -F "$kw" >/dev/null 2>&1; then HITS="$HITS $kw"; fi
      done
      if [ -n "$HITS" ]; then
        ck "⑫ 线上 404.html 清理残留复验（命中:$HITS）" false
      else
        ck "⑫ 线上 404.html 清理残留复验 0 命中" true
      fi
    else
      ck "⑫ 线上 404.html 清理复验——本次无清理类变更 SKIP" true
    fi
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

# ---------- 第 9/9 步：OTA 加密升级包（api/update/latest.json + downloads/*.enc）----------
# R-20261116-005：官网同步(第 8 步)成功后，若 secrets-vault\ota-keys.enc 存在则生成并
# 上传 OTA 产物；不存在则 ⚠ 跳过不失败（向后兼容：旧环境无 ota-keys.enc 仍可正常发布）。
echo "==> 9/9 OTA 加密升级包"
if [ -f "$VAULT/ota-keys.enc" ] && [ -f "$VAULT/key" ]; then
  [ "${SITE_OK:-0}" -eq 1 ] || { echo "  ⚠ 官网同步未完成，跳过 OTA 步骤（不阻断已完成的 1-8 步结果）"; exit 0; }
  # OTA-STEP FIX 20261126：第 8 步曾 cd 进已删除的临时目录 + B: 盘偶发抖动导致
  # wslpath 报 No such file（v0.5.1/0.5.2 两次实证，文件实际存在）——先回 ROOT、
  # wslpath/powershell 各带重试；最终失败降级为警告并提示手动补跑，不再 exit 1。
  cd "$ROOT" || true
  PS_OTA=""
  for _i in 1 2 3; do
    PS_OTA=$(wslpath -w "$ROOT/scripts/ota-package.ps1" 2>/dev/null) && [ -n "$PS_OTA" ] && break
    echo "  ⚠ wslpath 失败（第 $_i 次），2 秒后重试…"; sleep 2
  done
  OTA_OK=0
  if [ -n "$PS_OTA" ]; then
    for _i in 1 2 3; do
      powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$PS_OTA" -Version "$VER" \
        && { OTA_OK=1; break; } || { echo "  ⚠ ota-package 失败（第 $_i 次），3 秒后重试…"; sleep 3; }
    done
  fi
  LATEST_JSON="$ROOT/easyrdp-site/api/update/latest.json"
  OTA_ENC_FILE="$ROOT/easyrdp-site/downloads/EasyRDP-$VER-portable.exe.enc"
  # OTA-RETRY 20261128：生成失败先回退使用本地现存产物重传（post-XXX-ota-upload 模板语义），
  # 生成失败且本地无产物才终止并输出手动补跑指令。
  if [ "$OTA_OK" -ne 1 ]; then
    if [ -f "$LATEST_JSON" ] && [ -f "$OTA_ENC_FILE" ]; then
      echo "  ⚠ OTA 自动生成失败——回退：复用本地现存产物（latest.json + $(basename "$OTA_ENC_FILE")）继续上传"
    else
      echo "  ✗ OTA 包生成失败且本地无可用产物（latest.json / *.enc 均缺）"
      echo "    手动补跑指令（全部执行后重跑 release.sh --skip-build 仅补 OTA）："
      echo "      powershell -ExecutionPolicy Bypass -File scripts\\ota-package.ps1 -Version $VER"
      echo "      bash scripts/post-XXX-ota-upload.sh（参照 scripts/ota-upload-050.sh 模板改版本号）"
      rm -f "$KEY_PATH"; exit 1
    fi
  else
    echo "  ✓ latest.json + $(basename "$OTA_ENC_FILE") 已生成"
  fi

  # 第 8 步收尾已删除临时 SSH 私钥，此处重新准备
  KEY_PATH=/tmp/id_ed25519_easyssh
  cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH" \
    || { echo "  ✗ SSH 密钥准备失败"; exit 1; }
  ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "mkdir -p $SITE_DIR/api/update $SITE_DIR/downloads"
  UP_OK=0
  for _i in 1 2; do
    if scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$LATEST_JSON" "$SITE_HOST:$SITE_DIR/api/update/latest.json" \
       && scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$OTA_ENC_FILE" \
            "$SITE_HOST:$SITE_DIR/downloads/$(basename "$OTA_ENC_FILE")" \
       && ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" \
            "chown www:www $SITE_DIR/api/update/latest.json $SITE_DIR/downloads/$(basename "$OTA_ENC_FILE")"; then
      UP_OK=1; break
    fi
    echo "  ⚠ OTA 上传失败（第 $_i 次），5 秒后重试…"; sleep 5
  done
  rm -f "$KEY_PATH"
  if [ "$UP_OK" -eq 1 ]; then
    echo "  ✓ api/update/latest.json 与 downloads/$(basename "$OTA_ENC_FILE") 已上传"
  else
    echo "  ✗ OTA 上传重试后仍失败——手动补跑指令："
    echo "      powershell -ExecutionPolicy Bypass -File scripts\\ota-package.ps1 -Version $VER"
    echo "      参照 scripts/ota-upload-050.sh 模板 scp latest.json 与 .enc 后 chown www:www"
    exit 1
  fi

  # 8b 风格线上校验：latest.json 可访问才算完成（失败先重传一次再验）
  CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/api/update/latest.json")
  if [ "$CODE" != "200" ]; then
    echo "  ⚠ 线上 latest.json HTTP $CODE——重传一次后复验…"
    KEY_PATH=/tmp/id_ed25519_easyssh
    cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
    scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$LATEST_JSON" "$SITE_HOST:$SITE_DIR/api/update/latest.json" || true
    ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "chown www:www $SITE_DIR/api/update/latest.json" || true
    rm -f "$KEY_PATH"
    sleep 2
    CODE=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/api/update/latest.json")
  fi
  if [ "$CODE" = "200" ]; then
    echo "  ✓ 线上校验: https://easyrdp.cn/api/update/latest.json 可访问(200)"
    echo "  ✅ OTA 加密升级包已发布（v$VER）"
  else
    echo "  ✗ 线上校验: latest.json HTTP $CODE——OTA 步骤【未完成】，手动补跑指令："
    echo "      powershell -ExecutionPolicy Bypass -File scripts\\ota-package.ps1 -Version $VER"
    echo "      参照 scripts/ota-upload-050.sh 模板上传后 curl -s https://easyrdp.cn/api/update/latest.json 复验"
    exit 1
  fi
else
  echo "  ⚠ 无 secrets-vault\ota-keys.enc——跳过 OTA 加密升级包（向后兼容，不影响发布结果）"
fi
