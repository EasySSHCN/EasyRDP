#!/usr/bin/env bash
# =============================================================================
# 【已废弃 2026-11-17】本脚本为旧 push 方案（正式机→sv 推送），已被
# sv-pull.sh（sv→两台正式机拉取，部署在 sv:/opt/dsh/sv-standby/）取代。
# 现行方案与应急手册见 DNS-SWITCH-RUNBOOK.md。本文件仅留档，勿再部署。
#
# =============================================================================
# sync-to-sv.sh — 主服务器 → sv 备用服务器 rsync 同步核心脚本
#
# 用法（在【主服务器】上运行）:
#   ./sync-to-sv.sh --project easyrdp --full   # 首次全量
#   ./sync-to-sv.sh --project easyrdp --incr   # 增量（cron 用）
#   ./sync-to-sv.sh --project easyssh --full
#
# 说明:
#  - 【单向同步】只从主服务器 → sv.trae.show 推送。sv 服务器只进不出：
#    sv 端绝不回写主服务器；sv 端仅接收存储（建议 sv 侧密钥仅授权
#    restricted,command="..." 或内部-sftp 只读约束，见 DNS-SWITCH-RUNBOOK.md）。
#  - 配置从本目录 sv-sync.conf 读取（由 sv-sync.conf.example 复制填写）。
#  - rsync over ssh，加密通道；keys.php 含机密（API key/admin hash/token secret），
#    必须同步，加密通道传输可接受，但不落日志明文。
#  - 默认【不带 --delete】（谨慎模式），sv 上多余文件仅保留；--full 时启用
#    --delete-delay 白名单模式，仅删除已知可重建目录内的残留，不碰运行时数据。
#  - 两项目 webroot / 数据目录完全独立，互不覆盖（详见 CONFLICTS.md）。
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONF_FILE="${SCRIPT_DIR}/sv-sync.conf"
PROJECT=""
MODE="incr"
LOG_TAG="$(basename "$0")"

log() { echo "[$(date '+%F %T')] [$LOG_TAG] $*"; }
die() { echo "[$(date '+%F %T')] [$LOG_TAG][FATAL] $*" >&2; exit 1; }

usage() {
  grep '^#' "$0" | sed -n '2,20p'; exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT="${2:-}"; shift 2 ;;
    --full)    MODE="full";  shift ;;
    --incr)    MODE="incr";  shift ;;
    --conf)    CONF_FILE="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) die "未知参数: $1（--help 查看用法）" ;;
  esac
done

[[ -n "$PROJECT" ]] || die "必须指定 --project easyrdp|easyssh"
[[ -f "$CONF_FILE" ]] || die "缺少配置文件 $CONF_FILE（请先复制 sv-sync.conf.example 为 sv-sync.conf 并填写）"
# shellcheck source=sv-sync.conf
source "$CONF_FILE"

case "$PROJECT" in
  easyrdp)
    P_PRIMARY_HOST="$PRIMARY_EASYRDP_HOST"; P_PRIMARY_PORT="$PRIMARY_EASYRDP_PORT"
    P_PRIMARY_USER="$PRIMARY_EASYRDP_USER"; P_PRIMARY_KEY="$PRIMARY_EASYRDP_KEY"
    P_WEBROOT="$EASYRDP_WEBROOT";           P_DATA="$EASYRDP_DATA_DIR"
    P_SV_HOST="$SV_HOST"; P_SV_PORT="$SV_PORT"; P_SV_USER="$SV_USER"; P_SV_KEY="$SV_KEY"
    P_SV_WEBROOT="$SV_EASYRDP_WEBROOT";     P_SV_DATA="$SV_EASYRDP_DATA_DIR"
    ;;
  easyssh)
    P_PRIMARY_HOST="$PRIMARY_EASYSSH_HOST"; P_PRIMARY_PORT="$PRIMARY_EASYSSH_PORT"
    P_PRIMARY_USER="$PRIMARY_EASYSSH_USER"; P_PRIMARY_KEY="$PRIMARY_EASYSSH_KEY"
    P_WEBROOT="$EASYSSH_WEBROOT";           P_DATA="$EASYSSH_DATA_DIR"
    P_SV_HOST="$SV_HOST"; P_SV_PORT="$SV_PORT"; P_SV_USER="$SV_USER"; P_SV_KEY="$SV_KEY"
    P_SV_WEBROOT="$SV_EASYSSH_WEBROOT";     P_SV_DATA="$SV_EASYSSH_DATA_DIR"
    ;;
  *) die "--project 仅支持 easyrdp|easyssh" ;;
esac

for v in P_PRIMARY_HOST P_PRIMARY_PORT P_PRIMARY_USER P_PRIMARY_KEY \
         P_WEBROOT P_DATA P_SV_HOST P_SV_PORT P_SV_USER P_SV_KEY \
         P_SV_WEBROOT P_SV_DATA; do
  [[ -n "${!v}" ]] || die "配置项缺失: $v"
done
[[ -f "$P_PRIMARY_KEY" ]] || die "主服务器私钥不存在: $P_PRIMARY_KEY"
[[ -f "$P_SV_KEY" ]] || die "sv 服务器私钥不存在: $P_SV_KEY"

SSH_KEY_OPTS=(-i "$P_SV_KEY" -p "$SV_PORT" -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)

# ---------------------------------------------------------------------------
# 1. 预检: sv 可达 + 目标目录可写
# ---------------------------------------------------------------------------
log "预检 sv 连接 ${SV_USER}@${SV_HOST}:${SV_PORT} ..."
ssh "${SSH_KEY_OPTS[@]}" "${SV_USER}@${SV_HOST}" "mkdir -p '${P_SV_WEBROOT}' '${P_SV_DATA}'" \
  || die "sv 服务器不可达或目录创建失败"

# ---------------------------------------------------------------------------
# 2. 构建 rsync 排除清单（两项目通用 + 项目专属）
# ---------------------------------------------------------------------------
EXCLUDES=(
  # 永不删除/覆盖 sv 端本地配置
  --exclude='.user.ini'
  --exclude='*.log'
  --exclude='.well-known/'
)
# 项目专属排除（配置文件可覆盖为空）
if [[ "$PROJECT" == "easyrdp" ]]; then
  # shellcheck disable=SC2153
  read -r -a EXTRA <<< "${EASYRDP_EXCLUDES:-}"
else
  read -r -a EXTRA <<< "${EASYSSH_EXCLUDES:-}"
fi
EXCLUDES+=("${EXTRA[@]}")

RSYNC_SSH=(-e "ssh -i ${P_SV_KEY} -p ${SV_PORT} -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15")
RSYNC_COMMON=(-az --partial --timeout=180 --chmod=F600 "${EXCLUDES[@]}" "${RSYNC_SSH[@]}")

# 删除策略：谨慎模式
#  - incr: 不带 --delete，sv 端只增不减
#  - full: --delete-delay，但仅对白名单目录启用（排除掉运行时数据后再删）
DELETE_OPTS=()
if [[ "$MODE" == "full" ]]; then
  DELETE_OPTS=(--delete-delay)
fi

# ---------------------------------------------------------------------------
# 3. 同步 webroot（站点代码 + keys.php 机密 + downloads 产物）
#    注意: keys.php 位于 api/ 下, 必须同步, 不在排除清单中。
# ---------------------------------------------------------------------------
log "同步 webroot ${P_WEBROOT} → sv:${P_SV_WEBROOT} (mode=${MODE})"
rsync "${RSYNC_COMMON[@]}" "${DELETE_OPTS[@]}" \
  --exclude='feedback-db.json' --exclude='*.tmp.*' --exclude='data/stats-*' \
  "root@${P_PRIMARY_HOST}:${P_WEBROOT}/" \
  "${SV_USER}@${SV_HOST}:${P_SV_WEBROOT}/" \
  || die "webroot rsync 失败 (project=${PROJECT})"

# ---------------------------------------------------------------------------
# 4. 同步运行时数据目录（反馈库/统计库，webroot 之外）
# ---------------------------------------------------------------------------
log "同步数据目录 ${P_DATA} → sv:${P_SV_DATA}"
rsync "${RSYNC_COMMON[@]}" "${DELETE_OPTS[@]}" \
  "root@${P_PRIMARY_HOST}:${P_DATA}/" \
  "${SV_USER}@${SV_HOST}:${P_SV_DATA}/" \
  || die "数据目录 rsync 失败 (project=${PROJECT})"

# ---------------------------------------------------------------------------
# 5. 同步 letsencrypt 证书（保证切 DNS 后 HTTPS 可用）
#    目标路径与主服务器一致: /etc/letsencrypt
# ---------------------------------------------------------------------------
if [[ "${SYNC_LETSENCRYPT:-yes}" == "yes" ]]; then
  log "同步 /etc/letsencrypt → sv:/etc/letsencrypt"
  ssh "${SSH_KEY_OPTS[@]}" "${SV_USER}@${SV_HOST}" "mkdir -p /etc/letsencrypt"
  rsync "${RSYNC_SSH[@]}" -az --partial --timeout=180 \
    "${DELETE_OPTS[@]}" \
    --exclude='*.lock' --exclude='backups/' \
    "root@${P_PRIMARY_HOST}:/etc/letsencrypt/" \
    "${SV_USER}@${SV_HOST}:/etc/letsencrypt/" \
    || log "[WARN] letsencrypt 同步失败（不阻断本次同步，切换前须手动确认证书）"
fi

# ---------------------------------------------------------------------------
# 6. sv 端收尾: 修属主与权限（keys.php 600 / www:www）
# ---------------------------------------------------------------------------
log "sv 端收尾: 修属主与权限"
ssh "${SSH_KEY_OPTS[@]}" "${SV_USER}@${SV_HOST}" bash -s -- \
  "${P_SV_WEBROOT}" "${P_SV_DATA}" <<'REMOTE'
set -e
WEBROOT="$1"; DATA="$2"
chown -R www:www "$WEBROOT" "$DATA" 2>/dev/null || chown -R nginx:nginx "$WEBROOT" "$DATA"
chmod 750 "$DATA"
find "$WEBROOT/api" -name 'keys.php' -exec chmod 600 {} \; -exec chown root:www {} \; 2>/dev/null || true
REMOTE

log "完成: project=${PROJECT} mode=${MODE}"
