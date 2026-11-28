<?php
/**
 * GET /api/download/get.php（即 GET /api/download，见同目录 index.php 薄路由）
 * —— 签名 Token 中转下载：HMAC 校验 + 流式转发，绝不 302 暴露真实文件 URL（R-20261119-007）
 *
 * 请求：GET ?token=<id32hex>.<sig32hex>[&dl=<download_id32hex>]
 *   - token 由 POST api/download/token.php 签发（文件名不进 token，服务端 dl-tokens.json 查表映射）
 *   - dl 为续传会话：下载开始自动生成 download_id 会话；续传带 download_id+token，
 *     会话最后活动 +10 分钟内放行（common.php::dl_validate_and_book 统一记账）
 *
 * 校验链（任何失败不暴露真实文件，仅状态码/提示）：
 *   HMAC 签名（hash_equals）→ 滑动窗口（最后成功请求起 10 分钟）→
 *   IP 绑定（宽松 /24 网段，keys.php bind_ip 可关）→ 并发上限（同 token 16 连接，超限 429）
 *
 * 下发：流式转发（readfile 分块 + flush），不 302 重定向；
 *   Content-Disposition: attachment; filename="<原始文件名>"；Accept-Ranges 支持：
 *   单区间 Range 请求按 206 + Content-Range 分块应答（断点续传）。
 *   zip 包且 token 关联了归因票据（R-20261118-001）时沿用旧逻辑：票据有效则注入
 *   ticket.id（按票缓存于 easyrdp-data/tickets/packaged/），归因链路不断。
 * 每次成功下发记日志（IP/token/文件/时间 → dl-log.json）。
 */

require __DIR__ . '/common.php';

header('Cache-Control: no-store');

if (($_SERVER['REQUEST_METHOD'] ?? 'GET') !== 'GET') st_fail('请使用 GET', 405);

$token = trim((string)($_GET['token'] ?? ''));
$dl = trim((string)($_GET['dl'] ?? ''));
if ($token === '') st_fail('缺少 token', 400);

$now = st_now_ms();
$ip = fb_client_ip();
$rec = dl_validate_and_book($token, $dl); // 校验+记账（失败已 exit；含滑动窗口/IP 绑定/并发 429）

$file = (string)($rec['f'] ?? '');
if ($file === '' || preg_match('/^[A-Za-z0-9._-]+$/', $file) !== 1) st_fail('文件名非法', 400);
$origPath = dirname(__DIR__, 2) . '/downloads/' . $file;
if (!is_file($origPath) || filesize($origPath) <= 0) st_fail('文件暂不可用，请前往 GitHub Releases 获取', 404);

/* ---- 日志：每次成功下发（含 Range 续传）记一条 ---- */
dl_log_add(['ts' => $now, 'ip' => (string)$ip, 'ipm' => st_ip_mask($ip), 'file' => $file,
            'tid' => (string)(dl_token_parse($token)[0] ?? ''),
            'dl' => (string)($rec['dl_id'] ?? ''),
            'range' => substr((string)($_SERVER['HTTP_RANGE'] ?? ''), 0, 80), 'ev' => 'serve']);

/* ---- 分块流式输出 ---- */
function dl_stream_file($path, $offset, $length) {
  $fh = @fopen($path, 'rb');
  if (!$fh) st_fail('文件读取失败', 500);
  try {
    @fseek($fh, $offset);
    $left = $length;
    while ($left > 0 && !feof($fh)) {
      $chunk = @fread($fh, $left > 262144 ? 262144 : $left);
      if ($chunk === false || $chunk === '') break;
      $left -= strlen($chunk);
      echo $chunk;
      @flush(); @ob_flush();
    }
  } finally {
    @fclose($fh);
  }
}

$total = (int)filesize($origPath);

/* ---- zip + 归因票据：注入 ticket.id（沿用 get.php 旧逻辑；缓存复用） ---- */
$ticket = (string)($rec['ticket'] ?? '');
if ($ticket !== '' && strcasecmp(substr($file, -4), '.zip') === 0 && class_exists('ZipArchive')) {
  define('TKG_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data/tickets');
  $ticketOk = false;
  $tdb = st_db_load(TKG_DATA_DIR . '/tickets.json', 'ticket');
  $tRec = isset($tdb['tickets'][$ticket]) && is_array($tdb['tickets'][$ticket]) ? $tdb['tickets'][$ticket] : null;
  if ($tRec !== null && $now - (int)($tRec['ts'] ?? 0) <= 7 * 24 * 3600 * 1000) $ticketOk = true;

  if ($ticketOk) {
    $cachedDir = TKG_DATA_DIR . '/packaged/' . $ticket;
    $cachedZip = $cachedDir . '/' . $file;
    if (!is_file($cachedZip)) {
      if (!is_dir($cachedDir)) @mkdir($cachedDir, 0755, true);
      $z = new ZipArchive();
      $packOk = ($z->open($cachedZip, ZipArchive::CREATE | ZipArchive::OVERWRITE) === true
        && $z->addFile($origPath, $file) === true          // 原包原样放入根目录
        && $z->addFromString('ticket.id', $ticket . "\n")); // 解压后与 exe 同目录 → 客户端首报归因
      $z->close();
      if (!$packOk || !is_file($cachedZip) || filesize($cachedZip) <= 0) $ticketOk = false;
    }
    if ($ticketOk) {
      /* 票据标记 served（尽力而为） */
      st_locked_update(TKG_DATA_DIR . '/tickets.json', 'ticket', function ($db) use ($ticket, $file, $now) {
        if (isset($db['tickets'][$ticket]) && is_array($db['tickets'][$ticket])) {
          $db['tickets'][$ticket]['servedAt'] = $now;
          $db['tickets'][$ticket]['servedFile'] = $file;
        }
        return $db;
      });
      $origPath = $cachedZip; // 下发注入后的包（同流式通道，不 302）
      $total = (int)filesize($origPath);
    }
  }
}

/* ---- Range（单区间）断点续传 ---- */
header('Accept-Ranges: bytes');
header('Content-Type: application/octet-stream');
header('Content-Disposition: attachment; filename="' . $file . '"');
header('X-Content-Type-Options: nosniff');
header('X-Accel-Buffering: no');

$range = (string)($_SERVER['HTTP_RANGE'] ?? '');
if ($range !== '' && preg_match('/^bytes=(\d*)-(\d*)$/', $range, $m) === 1 && ($m[1] !== '' || $m[2] !== '')) {
  $start = $m[1] !== '' ? (int)$m[1] : max(0, $total - (int)$m[2]);
  $end = $m[1] !== '' ? ($m[2] !== '' ? min((int)$m[2], $total - 1) : $total - 1) : $total - 1;
  if ($start <= $end && $start < $total && $end >= 0) {
    http_response_code(206);
    header('Content-Range: bytes ' . $start . '-' . $end . '/' . $total);
    header('Content-Length: ' . (string)($end - $start + 1));
    dl_stream_file($origPath, $start, $end - $start + 1);
    exit;
  }
  /* 区间非法：416 */
  http_response_code(416);
  header('Content-Range: bytes */' . $total);
  exit;
}

header('Content-Length: ' . (string)$total);
dl_stream_file($origPath, 0, $total);
exit;
