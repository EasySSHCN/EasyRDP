<?php
/**
 * POST /api/download/token —— 签名 Token 下载：token 签发（R-20261119-007）
 *
 * download.html 下载按钮不再直链 downloads/ 真实文件；点击时前端先 POST 本接口：
 *   { file: "<文件名>", ticket?: "<归因票据id>" }
 *   → 白名单校验（白名单外 403）→ 可选 Referer/Origin 本站校验 → 同 IP 每分钟频控
 *   → 返回 { ok: true, url: "/api/download?token=...", expires_in: 600 }
 *
 * token 策略/存储/密钥惯例详见 common.php 头注与 keys.example.php。
 */

require __DIR__ . '/common.php';

header('Cache-Control: no-store');

if (($_SERVER['REQUEST_METHOD'] ?? 'GET') !== 'POST') st_fail('请使用 POST', 405);

$now = st_now_ms();
$ip = fb_client_ip();
$K = dl_keys();

/* ---- 可选 Referer/Origin 本站校验（头缺失放行，兼容 curl/另存为/下载器直开） ---- */
if ((bool)($K['check_referer'] ?? true)) {
  $ref = (string)($_SERVER['HTTP_REFERER'] ?? '');
  $org = (string)($_SERVER['HTTP_ORIGIN'] ?? '');
  $host = '';
  if ($org !== '') { $p = @parse_url($org); $host = (string)($p['host'] ?? ''); }
  elseif ($ref !== '') { $p = @parse_url($ref); $host = (string)($p['host'] ?? ''); }
  if ($host !== ''
      && preg_match('/(^|\.)easyrdp\.cn$/', $host) !== 1 && $host !== 'localhost' && $host !== '127.0.0.1') {
    dl_log_add(['ts' => $now, 'ip' => (string)$ip, 'ipm' => st_ip_mask($ip), 'file' => '',
                'tid' => '', 'ev' => 'deny_referer', 'host' => substr($host, 0, 120)]);
    st_fail('跨站请求被拒绝', 403);
  }
}

/* ---- 同 IP 每分钟申请频控（滑动 60s 窗口） ---- */
$rateMax = max(1, (int)($K['rate_per_min'] ?? 10));
$rateOk = false;
st_locked_update(DL_RATE_FILE, 'rate', function ($db) use (&$rateOk, $ip, $rateMax, $now) {
  if (!isset($db['hits']) || !is_array($db['hits'])) $db = ['hits' => [], 'meta' => ['created' => $now]];
  $key = hash('sha256', (string)$ip);
  $window = $now - 60 * 1000;
  $list = isset($db['hits'][$key]) && is_array($db['hits'][$key]) ? $db['hits'][$key] : [];
  $list = array_values(array_filter($list, function ($t) use ($window) { return $t > $window; }));
  if (count($list) >= $rateMax) { $db['hits'][$key] = $list; return $db; }
  $list[] = $now;
  $db['hits'][$key] = $list;
  if (count($db['hits']) > 10000) $db['hits'] = []; // 粗裁剪防膨胀
  $rateOk = true;
  return $db;
});
if (!$rateOk) st_fail('请求过于频繁，请稍后再试', 429);

/* ---- 解析请求体（JSON 优先，兼容表单） ---- */
$body = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($body)) $body = $_POST;
$file = trim((string)($body['file'] ?? ''));
$ticket = trim((string)($body['ticket'] ?? ''));

/* ---- 白名单校验：白名单外一律 403 ---- */
if ($file === '' || !in_array($file, dl_whitelist(), true)
    || preg_match('/^[A-Za-z0-9._-]+$/', $file) !== 1) {
  dl_log_add(['ts' => $now, 'ip' => (string)$ip, 'ipm' => st_ip_mask($ip),
              'file' => substr($file, 0, 120), 'tid' => '', 'ev' => 'deny_file']);
  st_fail('文件不在可下载清单内', 403);
}
$filePath = dirname(__DIR__, 2) . '/downloads/' . $file;
if (!is_file($filePath) || filesize($filePath) <= 0) {
  st_fail('文件暂不可用，请前往 GitHub Releases 获取', 404);
}

/* ---- 归因票据（可选，沿用 R-20261118-001 票据；格式合法即随 token 记账，
        zip 下发时由 get.php 注入 ticket.id；票据有效性在 get.php 侧再查票据库） ---- */
if ($ticket !== '' && preg_match('/^[0-9a-f]{16,64}$/', $ticket) !== 1) $ticket = '';

/* ---- 签发/复用：同「文件+网段+票据」10 分钟滑动窗内复用同 token ---- */
$tid = '';
$issued = st_locked_update(DL_TOKEN_FILE, 'token', function ($db) use (&$tid, $file, $ip, $ticket, $now) {
  if (!isset($db['tokens']) || !is_array($db['tokens'])) $db = ['tokens' => [], 'meta' => ['created' => $now]];
  $tokens = &$db['tokens'];
  $net = dl_ip_net($ip);
  foreach ($tokens as $k => $t) {
    if (!is_array($t) || $now - (int)($t['last'] ?? 0) > DL_TOKEN_WINDOW_MS) { unset($tokens[$k]); continue; }
  }
  foreach ($tokens as $k => $t) {
    if ((string)($t['f'] ?? '') === $file && (string)($t['net'] ?? '') === $net
        && (string)($t['ticket'] ?? '') === $ticket) {
      $t['last'] = $now;
      $tokens[$k] = $t;
      $tid = (string)$k;
      return $db;
    }
  }
  if (count($tokens) >= DL_TOKEN_MAX) {
    uasort($tokens, function ($a, $b) { return (int)($a['last'] ?? 0) <=> (int)($b['last'] ?? 0); });
    $tokens = array_slice($tokens, (int)(DL_TOKEN_MAX / 4), null, true);
  }
  $newId = bin2hex(random_bytes(16));
  $tokens[$newId] = ['f' => $file, 'net' => $net, 'ip' => (string)$ip,
                     'ticket' => $ticket, 'created' => $now, 'last' => $now, 'dl' => []];
  $tid = $newId;
  return $db;
});
if ($issued === null || $tid === '') st_fail('token 签发失败，请稍后再试', 500);

dl_log_add(['ts' => $now, 'ip' => (string)$ip, 'ipm' => st_ip_mask($ip), 'file' => $file,
            'tid' => $tid, 'ev' => 'issue']);

st_json_out([
  'ok' => true,
  'url' => '/api/download?token=' . dl_token_of($tid),
  'file' => $file,
  'expires_in' => (int)(DL_TOKEN_WINDOW_MS / 1000),
]);
