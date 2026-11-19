#!/usr/bin/env bash
# =============================================================================
# sync-cron-setup.sh — 在【主服务器】上注册定时 rsync 推送到 sv（单向）
# 用法（主服务器 root 执行）: bash sync-cron-setup.sh [easyrdp|easyssh|both]
#  - easyrdp: 每 30 分钟（:05/:35）推送 webroot + 数据 + /etc/letsencrypt
#  - easyssh: 每 30 分钟错峰（:20/:50）
#  - 幂等: 按 marker 注释识别已有条目，重复执行不重复注册。
# 注意: 仅注册「主→sv」方向的推送任务；绝不创建任何 sv→主 的回写任务。
# =============================================================================
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_FILE="${SCRIPT_DIR}/sv-sync.conf"
MARKER="# sv-sync-managed"

TARGET="${1:-both}"
case "$TARGET" in easyrdp|easyssh|both) ;; *) echo "用法: $0 [easyrdp|easyssh|both]" >&2; exit 1;; esac
[[ -f "$CONF_FILE" ]] || { echo "缺少 $CONF_FILE" >&2; exit 1; }
# shellcheck source=sv-sync.conf
source "$CONF_FILE"
[[ -n "$SV_HOST" && -f "$SV_KEY" ]] || { echo "请先填写 sv-sync.conf（SV_HOST / SV_KEY）" >&2; exit 1; }

LOGDIR="/var/log/sv-sync"
mkdir -p "$LOGDIR"

add_cron() {
  local schedule="$1" project="$2"
  local line="${schedule} root ${SCRIPT_DIR}/sync-to-sv.sh --project ${project} --incr >> ${LOGDIR}/${project}-sync.log 2>&1 ${MARKER}:${project}"
  if crontab -l 2>/dev/null | grep -Fq "${MARKER}:${project}"; then
    echo "[skip] ${project} 定时任务已存在"
  else
    (crontab -l 2>/dev/null; echo "$line") | crontab -
    echo "[ok] 注册 ${project}: ${line}"
  fi
}

# 错峰: easyrdp :05/:35, easyssh :20/:50
[[ "$TARGET" == "easyrdp" || "$TARGET" == "both" ]] && add_cron "5,35 * * * *" easyrdp
[[ "$TARGET" == "easyssh" || "$TARGET" == "both" ]] && add_cron "20,50 * * * *" easyssh

echo "当前 sv-sync 相关 crontab 条目:"
crontab -l 2>/dev/null | grep -F "$MARKER" || true
echo
echo "提示: 切换 DNS 后（sv 为主）应立即注销本机定时任务，防止主服务器继续向 sv 单向覆盖:"
echo "  crontab -l | grep -v '$MARKER' | crontab -"
