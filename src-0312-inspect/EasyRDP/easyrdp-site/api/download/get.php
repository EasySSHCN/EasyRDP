<?php
/**
 * GET /api/download/get.php?f=<文件名>&t=<ticket_id> —— 按票下发安装包（R-20261118-001 打包侧补全）
 *
 * 背景：客户端 telemetry.rs 只读 exe 同目录 ticket.id；此前下载链接仅拼 ?t=
 * 到静态文件上，静态服务器忽略参数，包内永远没有 ticket.id → 归因链路断裂，
 * admin「已产生设备的访客记录」恒为空。本接口补上最后一环：
 *
 *   - zip 包：校验票据后，将 ticket.id（内容 = ticket_id）注入包根目录，
 *     按票缓存于 easyrdp-data/tickets/packaged/<ticket_id>/ 下，流式下发。
 *     用户解压后 ticket.id 与 exe 同目录 → 客户端首报自动携带 → 归因生效。
 *   - exe / tar.gz 等单文件：服务端无法注入（exe 尾段方案需客户端配合），
 *     302 跳转原始文件，行为与旧版一致。
 *
 * 安全：f 仅允许 [A-Za-z0-9._-]+ 且必须存在于 downloads/；t 必须是有效票据
 * （存在于票据库且在 7 天归因窗内）；同票同文件缓存复用；不校验 IP（允许
 * 换网络续传）。票据上加 served 标记便于后台观察下发情况。
 */

require __DIR__ . '/../stats/common.php';

define('TKG_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data/tickets');
define('TKG_FILE', TKG_DATA_DIR . '/tickets.json');
define('TKG_WINDOW_MS', 7 * 24 * 3600 * 1000);

header('Cache-Control: no-store');

$f = (string)($_GET['f'] ?? '');
if (strpos($f, 'downloads/') === 0) $f = substr($f, strlen('downloads/')); // 前端传完整 href，剥前缀
$t = trim((string)($_GET['t'] ?? ''));

/* ---- 参数与票据校验：任何失败静默 302 原始文件，绝不阻断下载 ---- */
$origUrl = '../downloads/' . rawurlencode($f); // 相对本接口的静态路径
function tkg_fallback(string $url) { header('Location: ' . $url); exit; }

if ($f === '' || preg_match('/^[A-Za-z0-9._-]+$/', $f) !== 1 || strpos($f, '..') !== false) {
  tkg_fallback($origUrl);
}
$origPath = dirname(__DIR__, 2) . '/downloads/' . $f;
if (!is_file($origPath)) tkg_fallback($origUrl);

$ticketOk = false;
if ($t !== '' && preg_match('/^[0-9a-f]{16,64}$/', $t) === 1) {
  $tdb = st_db_load(TKG_FILE, 'ticket');
  $rec = isset($tdb['tickets'][$t]) && is_array($tdb['tickets'][$t]) ? $tdb['tickets'][$t] : null;
  if ($rec !== null && st_now_ms() - (int)($rec['ts'] ?? 0) <= TKG_WINDOW_MS) $ticketOk = true;
}
if (!$ticketOk) tkg_fallback($origUrl);

/* ---- zip：注入 ticket.id（缓存复用）；非 zip：302 原文件 ---- */
if (strcasecmp(substr($f, -4), '.zip') === 0 && class_exists('ZipArchive')) {
  $cachedDir = TKG_DATA_DIR . '/packaged/' . $t;
  $cachedZip = $cachedDir . '/' . $f;
  if (!is_file($cachedZip)) {
    if (!is_dir($cachedDir)) @mkdir($cachedDir, 0755, true);
    $z = new ZipArchive();
    if ($z->open($cachedZip, ZipArchive::CREATE | ZipArchive::OVERWRITE) === true) {
      if ($z->addFile($origPath, $f) === true) {          // 原包原样放入根目录
        $z->addFromString('ticket.id', $t . "\n");        // 解压后与 exe 同目录
      }
      $z->close();
      if (!is_file($cachedZip) || filesize($cachedZip) === 0) tkg_fallback($origUrl);
    } else {
      tkg_fallback($origUrl); // 注入失败静默降级
    }
  }
  /* 票据标记 served（尽力而为，失败不影响下发） */
  st_locked_update(TKG_FILE, 'ticket', function ($db) use ($t, $f) {
    if (isset($db['tickets'][$t]) && is_array($db['tickets'][$t])) {
      $db['tickets'][$t]['servedAt'] = st_now_ms();
      $db['tickets'][$t]['servedFile'] = (string)$f;
    }
    return $db;
  });
  header('Content-Type: application/zip');
  header('Content-Disposition: attachment; filename="' . $f . '"');
  header('Content-Length: ' . (string)filesize($cachedZip));
  header('X-Accel-Buffering: no');
  readfile($cachedZip);
  exit;
}

tkg_fallback($origUrl);
