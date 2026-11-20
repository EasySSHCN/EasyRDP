#!/usr/bin/env bash
# =============================================================================
# sv-pull.sh — sv 备用服务器从两台正式机【拉取】同步核心脚本（pull 模式）
#
# 运行位置: 仅在 sv.trae.show 上运行（/opt/dsh/sv-standby/）
# 用法:
#   ./sv-pull.sh --project easyrdp --full    # 首次全量
#   ./sv-pull.sh --project easyssh --incr    # 增量（cron 用）
#   ./sv-pull.sh --all --incr                # 两个项目顺序拉取
#   ./sv-pull.sh --install-cron              # 注册/更新本机拉取 cron（幂等）
#
# 设计（2026-11-17 用户定版，取代旧 push 方案 sync-to-sv.sh）:
#  - 【单向拉取】sv → 正式机只读拉取；sv 绝不回写正式机。
#    两个项目在 sv 上目录完全独立（easyssh.cn / easyrdp.cn / 两套数据目录），
#    由 sv 一台机器统一调度，避免双项目兼容放置冲突。
#  - 密钥面最小化: 只有 sv 持有专用拉取私钥 /root/.ssh/id_ed25519_svpull，
#    两台正式机的 authorized_keys 对应行带 restrict,from="<sv出口IP>" 限源。
#  - rsync over ssh；keys.php 机密必须同步（加密通道可接受，不落日志明文）。
#  - 增量不带 --delete（sv 只增不减）；--full 带 --delete-delay 白名单清理。
#  - 切 DNS 到 sv 后（应急启用），必须先停本机拉取 cron，防止覆盖 sv 热备期
#    产生的运行时数据（见 DNS-SWITCH-RUNBOOK.md §2）。
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_FILE="${SCRIPT_DIR}/sv-pull.conf"
PROJECT=""
MODE="incr"
LOG_TAG="$(basename "$0")"
LOG_DIR="/var/log/sv-sync"

log() { echo "[$(date '+%F %T')] [$LOG_TAG] $*"; }
die() { echo "[$(date '+%F %T')] [$LOG_TAG][FATAL] $*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT="${2:-}"; shift ;;
    --all)     PROJECT="all"; shift ;;
    --full)    MODE="full"; shift ;;
    --incr)    MODE="incr"; shift ;;
    --conf)    CONF_FILE="${2:-}"; shift ;;
    --install-cron) INSTALL_CRON=1; shift ;;
    -h|--help) grep '^#' "$0" | sed -n '2,22p'; exit 0 ;;
    *) die "未知参数: $1（--help 查看用法）" ;;
  esac
done

[[ -f "$CONF_FILE" ]] || die "缺少配置文件 $CONF_FILE（由 sv-pull.conf 生成，含服务器地址，勿外泄）"
# shellcheck source=sv-pull.conf
source "$CONF_FILE"

mkdir -p "$LOG_DIR"

# ---------------- cron 安装模式 ----------------
if [[ "${INSTALL_CRON:-0}" == "1" ]]; then
  CRON_MARKER="# sv-pull-managed"
  TMP="$(mktemp)"
  crontab -l 2>/dev/null | grep -v "$CRON_MARKER" > "$TMP" || true
  cat >> "$TMP" <<EOF
$CRON_PULL_CRON $CRON_MARKER
$CRON_PULL_CRON2 $CRON_MARKER
EOF
  crontab "$TMP" && rm -f "$TMP"
  log "cron 已注册:"
  crontab -l | grep "$CRON_MARKER"
  exit 0
fi

[[ -n "$PROJECT" ]] || die "必须指定 --project easyrdp|easyssh 或 --all"

run_one() {
  local proj="$1"
  local P_HOST P_PORT P_USER P_WEBROOT P_DATA P_SV_WEBROOT P_SV_DATA CERT_DIR SV_CERT_DIR
  case "$proj" in
    easyrdp)
      P_HOST="$PRIMARY_EASYRDP_HOST"; P_PORT="$PRIMARY_EASYRDP_PORT"; P_USER="$PRIMARY_EASYRDP_USER"
      P_WEBROOT="$EASYRDP_WEBROOT";   P_DATA="$EASYRDP_DATA_DIR"
      P_SV_WEBROOT="$SV_EASYRDP_WEBROOT"; P_SV_DATA="$SV_EASYRDP_DATA_DIR"
      CERT_DIR="$EASYRDP_CERT_DIR";   SV_CERT_DIR="$SV_EASYRDP_CERT_DIR"
      read -r -a EXTRA <<< "${EASYRDP_EXCLUDES:-}"
      ;;
    easyssh)
      P_HOST="$PRIMARY_EASYSSH_HOST"; P_PORT="$PRIMARY_EASYSSH_PORT"; P_USER="$PRIMARY_EASYSSH_USER"
      P_WEBROOT="$EASYSSH_WEBROOT";   P_DATA="$EASYSSH_DATA_DIR"
      P_SV_WEBROOT="$SV_EASYSSH_WEBROOT"; P_SV_DATA="$SV_EASYSSH_DATA_DIR"
      CERT_DIR="$EASYSSH_CERT_DIR";   SV_CERT_DIR="$SV_EASYSSH_CERT_DIR"
      read -r -a EXTRA <<< "${EASYSSH_EXCLUDES:-}"
      ;;
    *) die "未知项目: $proj" ;;
  esac

  [[ -f "$PULL_KEY" ]] || die "拉取私钥不存在: $PULL_KEY"
  local SSH_OPTS=(-i "$PULL_KEY" -p "$P_PORT" -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)
  local RSYNC_SSH=(-e "ssh -i ${PULL_KEY} -p ${P_PORT} -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15")

  log "[$proj] 预检主服务器 ${P_USER}@${P_HOST}:${P_PORT} ..."
  ssh "${SSH_OPTS[@]}" "${P_USER}@${P_HOST}" "test -d '$P_WEBROOT' && test -d '$P_DATA'" \
    || die "[$proj] 主服务器不可达或目录缺失"

  local EXCLUDES=(--exclude='.user.ini' --exclude='*.log' --exclude='.well-known/' "${EXTRA[@]}")
  local COMMON=(-az --partial --timeout=600 --chmod=F600 "${EXCLUDES[@]}" "${RSYNC_SSH[@]}")
  local DELETE_OPTS=()
  [[ "$MODE" == "full" ]] && DELETE_OPTS=(--delete-delay)

  log "[$proj] 拉取 webroot ${P_USER}@${P_HOST}:${P_WEBROOT} → sv:${P_SV_WEBROOT} (mode=${MODE})"
  rsync "${COMMON[@]}" "${DELETE_OPTS[@]}" \
    --exclude='feedback-db.json' --exclude='*.tmp.*' --exclude='data/stats-*' \
    "${P_USER}@${P_HOST}:${P_WEBROOT}/" "${P_SV_WEBROOT}/" \
    || die "[$proj] webroot 拉取失败"

  log "[$proj] 拉取数据目录 ${P_USER}@${P_HOST}:${P_DATA} → sv:${P_SV_DATA}"
  rsync "${COMMON[@]}" "${DELETE_OPTS[@]}" \
    "${P_USER}@${P_HOST}:${P_DATA}/" "${P_SV_DATA}/" \
    || die "[$proj] 数据目录拉取失败"

  log "[$proj] 拉取 HTTPS 证书 ${CERT_DIR} → sv:${SV_CERT_DIR}"
  if [[ "${SYNC_LETSENCRYPT:-yes}" == "yes" ]]; then
    mkdir -p "$SV_CERT_DIR"
    rsync "${RSYNC_SSH[@]}" -az --partial --timeout=600 \
      --exclude='*.lock' --exclude='backups/' \
      "${P_USER}@${P_HOST}:${CERT_DIR}/" "${SV_CERT_DIR}/" \
      || log "[$proj][WARN] 证书拉取失败（不阻断，切 DNS 前须手动确认证书）"
  fi

  log "[$proj] 收尾: 修属主与权限"
  set +e
  chown -R www:www "$P_SV_WEBROOT" "$P_SV_DATA" 2>/dev/null || chown -R nginx:nginx "$P_SV_WEBROOT" "$P_SV_DATA"
  set -e
  chmod 750 "$P_SV_DATA"
  find "$P_SV_WEBROOT/api" -name 'keys.php' -exec chmod 600 {} \; -exec chown root:www {} \; 2>/dev/null || true

  log "[$proj] 完成: mode=${MODE}"
}

case "$PROJECT" in
  all) run_one easyrdp; run_one easyssh ;;
  *)   run_one "$PROJECT" ;;
esac
log "全部完成"
