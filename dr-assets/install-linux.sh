#!/usr/bin/env bash
# ============================================================================
# EasyRDP 开发机角色一键安装脚本（Linux 环境，Ubuntu 22.04/24.04 或 Debian 12）
# ----------------------------------------------------------------------------
# 用途：在 Linux 开发机上复刻 EasyRDP 项目的 Linux 侧开发环境：
#       Rust 工具链 + Tauri Linux 依赖 + FreeRDP2 系统库 + PowerShell + Node.js
#       + DSH 开发角色 + 定时任务（反馈采集/发布巡检，pwsh 执行）
# 用法：sudo bash install-linux.sh [--project=/opt/EasyRDP] [--skip-dsh]
# 说明：Windows 专属构建链（MSVC/vcpkg 静态 FreeRDP）不在本脚本范围——那是
#       install-windows.ps1 的职责；Linux 侧使用系统 FreeRDP2 开发包。
# ============================================================================
set -euo pipefail

PROJECT='/opt/EasyRDP'
SKIP_DSH=0
for a in "$@"; do
  case "$a" in
    --project=*) PROJECT="${a#--project=}" ;;
    --skip-dsh)  SKIP_DSH=1 ;;
  esac
done
[ "$(id -u)" -eq 0 ] || { echo "必须以 root 运行"; exit 1; }
PKGDIR="$(cd "$(dirname "$0")" && pwd)"
RUN_USER="${SUDO_USER:-root}"

echo "== [1/8] 系统包：编译链 / Tauri Linux 依赖 / FreeRDP2 开发库 =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y \
  build-essential pkg-config curl git unzip \
  libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libfreerdp2-dev libfreerdp-client2-dev libwinpr2-dev \
  wget gnupg ca-certificates software-properties-common

echo "== [2/8] PowerShell（Microsoft 仓库）=="
if ! command -v pwsh >/dev/null 2>&1; then
  curl -sSL "https://packages.microsoft.com/keys/microsoft.asc" | gpg --dearmor > /usr/share/keyrings/microsoft.gpg
  echo "deb [signed-by=/usr/share/keyrings/microsoft.gpg] https://packages.microsoft.com/config/$(. /etc/os-release && echo ID)/$(. /etc/os-release && echo VERSION_ID)/ prod" \
    > /etc/apt/sources.list.d/microsoft.list
  apt-get update -qq && apt-get install -y powershell
fi

echo "== [3/8] Node.js LTS（NodeSource）=="
if ! command -v node >/dev/null 2>&1; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
  apt-get install -y nodejs
fi

echo "== [4/8] Rust 工具链（rustup）=="
if ! command -v rustup >/dev/null 2>&1; then
  su - "$RUN_USER" -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable'
fi

echo "== [5/8] 部署项目源码 → $PROJECT =="
[ -d "$PKGDIR/EasyRDP" ] || { echo "缺少 EasyRDP 源码目录"; exit 1; }
mkdir -p "$(dirname "$PROJECT")"
mkdir -p "$PROJECT"
cp -a "$PKGDIR/EasyRDP/." "$PROJECT/"
chown -R "$RUN_USER":"$RUN_USER" "$PROJECT" 2>/dev/null || true

echo "== [6/8] 修正脚本内硬编码路径（B:\\... → $PROJECT）=="
OLD_ESC='B:\\Users\\ZhenAn\\Documents\\deepseek-harness\\default-workspace\\EasyRDP'
grep -rl "$OLD_ESC" "$PROJECT/scripts" "$PROJECT/make-release-assets.ps1" 2>/dev/null | while read -r f; do
  sed -i "s|$OLD_ESC|$PROJECT|g" "$f"
  echo "  patched: $f"
done
OLD_FWD='B:/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP'
grep -rl "$OLD_FWD" "$PROJECT/docs" 2>/dev/null | while read -r f; do
  sed -i "s|$OLD_FWD|$PROJECT|g" "$f"
done

echo "== [7/8] 定时任务（crontab）：反馈采集 08:00/20:00 + 发布巡检 04:00 =="
( crontab -u "$RUN_USER" -l 2>/dev/null || true; \
  echo "0 8,20 * * * pwsh -NoProfile -File $PROJECT/scripts/harvest-feedback.ps1 >> $PROJECT/reqpool/harvest.cron.log 2>&1"; \
  echo "0 4 * * * pwsh -NoProfile -File $PROJECT/scripts/release-check.ps1 >> $PROJECT/reqpool/release-check.cron.log 2>&1" ) \
  | crontab -u "$RUN_USER" -
echo "  ✓ 已写入 crontab（用户 $RUN_USER）"

echo "== [8/8] DeepSeek Harness（DSH 开发角色）=="
if [ "$SKIP_DSH" -eq 1 ]; then
  echo "  已跳过（--skip-dsh）"
else
  cat <<DSH_NOTE
DSH（DeepSeek Harness）Linux 侧安装（开发角色）：
  1) 从 DSH 官方发布渠道（官网/GitHub Releases）下载 Linux 发行包
     （AppImage/deb/tar），安装到系统或用户目录；
  2) 启动 DSH Web GUI，工作目录设为：$PROJECT
  3) 在 DSH 内重建「提示型」定时任务（发布巡检 04:00 / 反馈采集复审）——
     本脚本第 7 步已建立直接执行型 crontab，两者择一即可；
  4) 若从原开发机迁移：打包其 DSH 安装目录 + profile（会话/插件/技能）整体复制。
DSH_NOTE
fi

echo ""
echo "== 自检 =="
su - "$RUN_USER" -c '
  source "$HOME/.cargo/env" 2>/dev/null || true
  rustc --version && cargo --version && node --version && pwsh --version
  echo "（可选）在 easyrdp-client 下执行：cargo check -p easyrdp-rdp --features native"
'
echo ""
echo "✅ 开发机 Linux 环境安装完成：$PROJECT"
