<?php
/**
 * api/download/common.php —— 签名 Token 下载公共层（R-20261119-007）
 *
 * 供 token.php（签发）与 get.php（流式下发）共用：
 *   - 白名单 = download.html / en/download.html 实际存在文件清单（并集）
 *   - keys.php 加载惯例（PHP 退出守卫 + JSON，真实密钥绝不入库，同 api/gateway）
 *   - token = "<id32hex>.<sig32hex>"，HMAC-SHA256 签名，文件名不进 token
 *   - 存储：/www/wwwroot/easyrdp-data/downloads/（webroot 之外，HTTP 不可直达）
 *     dl-tokens.json（token 记账）、dl-log.json（下载日志）、dl-token-ratelimit.json（申请频控）
 *   - 复用 api/stats/common.php 的 st_locked_update / st_fail / st_now_ms 与
 *     api/feedback/common.php 的 fb_client_ip / st_ip_mask（经 stats/common require_once 引入）
 */

require_once __DIR__ . '/../stats/common.php';

define('DL_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data/downloads');
define('DL_TOKEN_FILE', DL_DATA_DIR . '/dl-tokens.json');
define('DL_LOG_FILE', DL_DATA_DIR . '/dl-log.json');
define('DL_RATE_FILE', DL_DATA_DIR . '/dl-token-ratelimit.json');
define('DL_TOKEN_WINDOW_MS', 10 * 60 * 1000); // 滑动窗口：最后一次成功请求起 10 分钟
define('DL_TOKEN_MAX', 5000);                 // 在册 token 上限（按 last 清理/裁剪，防灌水）
define('DL_LOG_MAX', 3000);                   // 下载日志保留条数上限
define('DL_DL_SESSION_MS', 10 * 60 * 1000);   // download_id 会话：最后活动 +10 分钟内放行
define('DL_MAX_CONNS', 16);                   // 同 token 最多并发连接（超过 429）

/** download.html 实际存在文件清单（中/英 download.html 全部下载链接文件名的并集） */
function dl_whitelist() {
  return [
    'EasyRDP-0.4.1-portable.exe',
    'EasyRDP-0.4.1-win32-x64-portable.zip',
    'EasyRDP-src-0.4.1.tar.gz',
    'EasyRDP-Gateway-0.4.1-win-x64.zip',
    'EasyRDP-0.4.0-portable.exe',
    'EasyRDP-0.4.0-win32-x64-portable.zip',
    'EasyRDP-src-0.4.0.tar.gz',
    'EasyRDP-Gateway-0.4.0-win-x64.zip',
    'EasyRDP-0.3.13-portable.exe',
    'EasyRDP-0.3.13-win32-x64-portable.zip',
    'EasyRDP-Gateway-0.3.13-win-x64.zip',
    'EasyRDP-src-0.3.13.tar.gz',
    'EasyRDP-0.3.12-portable.exe',
    'EasyRDP-0.3.12-win32-x64-portable.zip',
    'EasyRDP-Gateway-0.3.12-win-x64.zip',
    'EasyRDP-src-0.3.12.tar.gz',
    'EasyRDP-0.3.4-portable.exe',
    'EasyRDP-0.3.4-win32-x64-portable.zip',
    'EasyRDP-src-0.3.4.tar.gz',
  ];
}

/** 加载下载签名配置（keys.php 人工放置；缺失/损坏 = 未正确部署 → 500） */
function dl_keys() {
  static $keys = null;
  if ($keys !== null) return $keys;
  $keysFile = __DIR__ . '/keys.php';
  if (!is_readable($keysFile)) st_fail('下载签名配置缺失（api/download/keys.php 未放置），请联系管理员', 500);
  $lines = file($keysFile, FILE_IGNORE_NEW_LINES);
  if (!$lines || count($lines) < 2) st_fail('下载签名配置损坏，请联系管理员', 500);
  $keys = json_decode(implode("\n", array_slice($lines, 1)), true);
  $key = is_array($keys) ? (string)($keys['dl_sign_key'] ?? '') : '';
  if (strlen($key) < 32) st_fail('下载签名配置损坏，请联系管理员', 500);
  return $keys;
}

/** 生成/校验用 token 串：id32hex + HMAC 前 32 hex；文件名不进 token，服务端查表映射 */
function dl_token_of($id) {
  return $id . '.' . substr(hash_hmac('sha256', 'dlv1:' . (string)$id, (string)dl_keys()['dl_sign_key']), 0, 32);
}

/** 解析 token 串 → [id, sig]，格式非法返回 null */
function dl_token_parse($token) {
  $m = [];
  if (preg_match('/^([0-9a-f]{32})\.([0-9a-f]{32})$/', (string)$token, $m) !== 1) return null;
  return [$m[1], $m[2]];
}

/** 申请者 IP 宽松网段：IPv4 /24；IPv6 前 4 组（/64） */
function dl_ip_net($ip) {
  $b = @inet_pton((string)$ip);
  if ($b === false) return (string)$ip;
  if (strlen($b) === 4) return (string)long2ip(ip2long((string)$ip) & 0xFFFFFF00);
  $hex = unpack('H*', substr($b, 0, 4));
  return implode(':', str_split($hex[1], 4));
}

/** 下载日志追加（flock 内裁剪；写失败静默，不影响下发） */
function dl_log_add($event) {
  st_locked_update(DL_LOG_FILE, 'log', function ($db) use ($event) {
    if (!isset($db['log']) || !is_array($db['log'])) $db = ['log' => [], 'meta' => ['created' => st_now_ms()]];
    $db['log'][] = $event;
    if (count($db['log']) > DL_LOG_MAX) $db['log'] = array_slice($db['log'], -DL_LOG_MAX);
    return $db;
  });
}

/**
 * token 校验 + 记账（flock 内一次完成）：滑动窗口 / IP 绑定 / 并发上限 / download_id 会话。
 * 返回 token 记录（含 f/ticket）；失败时已输出错误并 exit（400/403/410/429）。
 * $dl: 客户端续传携带的 download_id（32hex），缺省自动新建会话（下载开始生成 download_id）。
 */
function dl_validate_and_book($token, $dl) {
  $K = dl_keys();
  $now = st_now_ms();
  $ip = fb_client_ip();
  $parsed = dl_token_parse($token);
  if ($parsed === null) st_fail('token 格式非法', 400);
  list($id, $sig) = $parsed;
  if (!hash_equals(dl_token_of($id), $id . '.' . $sig)) st_fail('token 签名校验失败', 403);
  if ($dl !== '' && preg_match('/^[0-9a-f]{16,64}$/', $dl) !== 1) $dl = '';

  $bindIp = (bool)($K['bind_ip'] ?? true);
  $net = dl_ip_net($ip);

  $rec = null;
  $out = st_locked_update(DL_TOKEN_FILE, 'token', function ($db) use (&$rec, $id, $now, $ip, $net, $bindIp, $dl) {
    if (!isset($db['tokens']) || !is_array($db['tokens'])) $db = ['tokens' => [], 'meta' => ['created' => $now]];
    $tokens = &$db['tokens'];
    /* 滑动窗口清理（含 download_id 会话簿） */
    foreach ($tokens as $k => $t) {
      if (!is_array($t)) { unset($tokens[$k]); continue; }
      if ($now - (int)($t['last'] ?? 0) > DL_TOKEN_WINDOW_MS) { unset($tokens[$k]); continue; }
      $dlMap = isset($t['dl']) && is_array($t['dl']) ? $t['dl'] : [];
      foreach ($dlMap as $dk => $dv) {
        if ($now - (int)$dv > DL_DL_SESSION_MS) unset($dlMap[$dk]);
      }
      $t['dl'] = $dlMap;
      $tokens[$k] = $t;
    }
    if (!isset($tokens[$id]) || !is_array($tokens[$id])) { st_fail('token 已过期，请回下载页重新获取', 410); }
    $t = $tokens[$id];
    /* IP 绑定（宽松 /24 网段；bind_ip 可关） */
    if ($bindIp && (string)($t['net'] ?? '') !== $net) { st_fail('token 与申请网段不符', 403); }
    /* 并发记账：download_id 已存在 → 续传放行并刷新活动；否则受 16 连接上限约束 */
    $dlMap = isset($t['dl']) && is_array($t['dl']) ? $t['dl'] : [];
    if ($dl !== '') {
      if (isset($dlMap[$dl])) {
        if ($now - (int)$dlMap[$dl] > DL_DL_SESSION_MS) { st_fail('下载会话已过期，请回下载页重新获取', 410); }
        $dlMap[$dl] = $now;
      } else {
        if (count($dlMap) >= DL_MAX_CONNS) { st_fail('该下载链接并发连接已达上限，请稍后再试', 429); }
        $dlMap[$dl] = $now;
      }
    } else {
      if (count($dlMap) >= DL_MAX_CONNS) { st_fail('该下载链接并发连接已达上限，请稍后再试', 429); }
      $dl = bin2hex(random_bytes(16));
      $dlMap[$dl] = $now;
    }
    /* 滑动窗口：有效期从最后一次成功请求起算 */
    $t['last'] = $now;
    $t['dl'] = $dlMap;
    $tokens[$id] = $t;
    $rec = $t;
    $rec['dl_id'] = $dl;
    return $db;
  });
  if ($out === null || $rec === null) st_fail('token 校验失败，请重试', 500);
  return $rec;
}
