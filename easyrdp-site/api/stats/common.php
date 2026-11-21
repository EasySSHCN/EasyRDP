<?php
/**
 * EasyRDP 官网后台统计 — 公共函数（api/stats/）
 *
 * 复用 api/feedback/common.php 的令牌鉴权（fb_check_token）与原子写工具（fb_json_file_*），
 * 管理员账号/密码/token 与反馈后台完全一致（keys.php 的 admin_user/admin_hash/token_secret）。
 *
 * 数据目录：__DIR__ = /www/wwwroot/easyrdp.cn/api/stats
 *   → dirname(__DIR__, 3) = /www/wwwroot → 数据落 /www/wwwroot/easyrdp-data/stats/（HTTP 不可直达）
 *
 * 存储（JSON 小文件，flock 并发锁 + tmp/rename 原子写）：
 *   client-db.json   客户端统计库（遥测上报 + 反馈系统信息聚合只读源）
 *   visit-db.json    官网访客统计库（beacon 写入，语义对齐 EasySSH v5.1.3+）
 *   updates-log.json 官网更新记录（check.php 轻量追加）
 *
 * v2（ADMIN-FULLPORT 2026-11-18）：
 *   - 访客流水行补齐 EasySSH 语义字段：id（流水唯一标识）、f（完整 IP）、rf（完整来源链接）；
 *     按日新增 ipsFull 完整 IP 明细桶（{IP:{c,last}}），与 EasySSH「完整 IP 明细/明细弹窗」对齐。
 *   - 新增 st_region() 归属地占位（EasyRDP 暂不接入外部归属地 API，返回 ''，后续可在此接入查询链）。
 *   - 保留 client.php / updates.php / visit-beacon.js 依赖的全部旧函数签名不变。
 */

define('ST_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data/stats');
define('ST_CLIENT_DB', ST_DATA_DIR . '/client-db.json');
define('ST_VISIT_DB', ST_DATA_DIR . '/visit-db.json');
define('ST_UPDATES_LOG', ST_DATA_DIR . '/updates-log.json');
define('ST_RATE_FILE', ST_DATA_DIR . '/stats-ratelimit.json');   // 遥测上报频控
define('ST_VISIT_RATE_FILE', ST_DATA_DIR . '/visit-ratelimit.json'); // 访客打点频控
define('ST_RATE_PER_HOUR', 30);
define('ST_VISIT_RATE_PER_HOUR', 600);
define('ST_RATE_MAX_KEYS', 10000);
define('ST_VISIT_RECENT_MAX', 1000);  // 最近访客流水保留条数（与 EasySSH VST_RECENT_MAX 对齐）
define('ST_VISIT_BUCKET_MAX', 2000);  // 单日 pages/refs/ips 桶上限
define('ST_VISIT_UV_MAX', 20000);     // 单日 UV 哈希集合上限
define('ST_VISIT_IPS_FULL_MAX', 5000);// 单日完整 IP 明细上限（EasySSH VST_IPS_FULL_MAX 同值）
define('ST_VISIT_DAYS_KEEP', 181);    // 按日数据保留天数
define('ST_UV_SALT', 'easyrdp-visit-uv-v1');
define('ST_LOG_MAX', 500);            // 客户端提交流水上限
define('ST_UPDATES_MAX', 5000);       // 更新记录流水上限
define('ST_MAX_VALUE', 160);          // 遥测单字段值上限
// ADMIN-FULLPORT：与 EasySSH 对齐的看板聚合参数
define('ST_ACTIVITY_KEEP_DAYS', 181); // 每设备按日活跃计数保留天数
define('ST_STAT_CARDS', 3);

require_once __DIR__ . '/../feedback/common.php';

function st_json_out($obj, $code = 200) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($obj, JSON_UNESCAPED_UNICODE);
  exit;
}

function st_fail($msg, $code = 400) { st_json_out(['ok' => false, 'message' => $msg], $code); }

function st_now_ms() { return (int)(microtime(true) * 1000); }

function st_db_default($kind) {
  if ($kind === 'client') return ['clients' => [], 'activity' => [], 'log' => [],
    'meta' => ['created' => st_now_ms(), 'note' => '匿名客户端统计,仅统计用途']];
  if ($kind === 'visit') return ['days' => [], 'recent' => [],
    'meta' => ['created' => st_now_ms(), 'note' => '官网访客统计,仅统计用途']];
  return ['events' => [], 'meta' => ['created' => st_now_ms(), 'note' => '官网更新记录']];
}

/** 读统计库（损坏/缺失回退默认结构） */
function st_db_load($path, $kind) {
  if (!is_readable($path)) return st_db_default($kind);
  $j = json_decode((string)file_get_contents($path), true);
  if (!is_array($j)) return st_db_default($kind);
  return array_merge(st_db_default($kind), $j);
}

/** 原子写统计库（tmp + rename，与 feedback 同模式） */
function st_db_save($path, $data) {
  $dir = dirname($path);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = $path . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($data, JSON_UNESCAPED_UNICODE), LOCK_EX) === false) {
    st_fail('服务器存储写入失败', 500);
  }
  @chmod($tmp, 0664);
  if (!@rename($tmp, $path)) { @unlink($tmp); st_fail('服务器存储写入失败', 500); }
}

/**
 * flock 并发安全的「读-改-写」：对 $path.lock 独占锁内执行 $fn($db) 后落盘。
 * 高并发写入入口（beacon / check.php 追加）必须走这里，避免读改写竞态丢数据。
 * 返回 $fn 的返回值；任何异常静默返回 null（统计写失败不影响主流程）。
 */
function st_locked_update($path, $kind, $fn) {
  $dir = dirname($path);
  if (!@is_dir($dir)) @mkdir($dir, 02775, true);
  $lockPath = $path . '.lock';
  $fh = @fopen($lockPath, 'c');
  if (!$fh) return null;
  $got = @flock($fh, LOCK_EX);
  if (!$got) { @fclose($fh); return null; }
  try {
    $db = st_db_load($path, $kind);
    $db = $fn($db);
    if ($db !== null) st_db_save($path, $db);
    $out = $db;
  } catch (Throwable $e) { $out = null; }
  @flock($fh, LOCK_UN);
  @fclose($fh);
  return $out;
}

/** 同 IP 滑动小时窗口频控；返回 true=放行 false=超限（静默，不输出错误）。 */
function st_rate_ok($file, $ip, $max) {
  $now = st_now_ms();
  $window = $now - 3600 * 1000;
  $ok = false;
  st_locked_update($file, 'rate', function ($db) use (&$ok, $ip, $max, $window) {
    if (!isset($db['hits']) || !is_array($db['hits'])) $db['hits'] = [];
    $key = hash('sha256', (string)$ip);
    $list = isset($db['hits'][$key]) && is_array($db['hits'][$key]) ? $db['hits'][$key] : [];
    $list = array_values(array_filter($list, function ($t) use ($window) { return $t > $window; }));
    if (count($list) < $max) { $list[] = st_now_ms(); $ok = true; }
    $db['hits'][$key] = $list;
    foreach ($db['hits'] as $k => $arr) {
      $db['hits'][$k] = array_values(array_filter(is_array($arr) ? $arr : [], function ($t) use ($window) { return $t > $window; }));
      if (!count($db['hits'][$k])) unset($db['hits'][$k]);
    }
    if (count($db['hits']) > ST_RATE_MAX_KEYS) {
      $keys = array_keys($db['hits']);
      shuffle($keys);
      $db['hits'] = array_intersect_key($db['hits'], array_flip(array_slice($keys, 0, ST_RATE_MAX_KEYS)));
    }
    return $db;
  });
  return $ok;
}

/** 客户端 IP 单向哈希（不存明文；盐复用 feedback token_secret） */
function st_ip_hash($ip) {
  return hash('sha256', (string)$ip . '|' . (string)(fb_keys()['token_secret'] ?? ''));
}

/** IP 掩码形态（仅旧数据兼容参考）：IPv4 → a.b.x.x；IPv6 → 前两 hextet + ::/32
 *  ADMIN-FULLIP（2026-11-18）：按项目持有者要求改为完整 IP 明文。新记录的 ip/f 字段均存完整 IP，
 *  本函数仅保留给 detail type=ip 行的 mask 参考字段与旧数据回退展示，不再用于新流水写入。 */
function st_ip_mask($ip) {
  $ip = (string)$ip;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $seg = explode('.', $ip);
    return $seg[0] . '.' . $seg[1] . '.x.x';
  }
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV6 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $h = bin2hex((string)@inet_pton($ip));
    return substr($h, 0, 4) . ':' . substr($h, 4, 4) . '::/32';
  }
  return '内网/未知';
}

/** 当日 UV 哈希：sha256(当日盐 + IP + UA)，按日轮换不可跨日追踪（EasySSH 同语义） */
function st_uv_hash($ip, $ua) {
  return hash('sha256', date('Y-m-d') . '|' . ST_UV_SALT . '|' . $ip . '|' . $ua);
}

/** UA → 浏览器家族（只取家族名，降低指纹粒度） */
function st_ua_family($ua) {
  $ua = (string)$ua;
  if (stripos($ua, 'Edg/') !== false) return 'Edge';
  if (stripos($ua, 'OPR/') !== false) return 'Opera';
  if (stripos($ua, 'Chrome') !== false && stripos($ua, 'Safari') !== false) return 'Chrome';
  if (stripos($ua, 'Chrome') !== false) return 'Chrome';
  if (stripos($ua, 'Safari') !== false) return 'Safari';
  if (stripos($ua, 'Firefox') !== false) return 'Firefox';
  if (stripos($ua, 'MSIE') !== false || stripos($ua, 'Trident/') !== false) return 'IE';
  if (stripos($ua, 'bot') !== false || stripos($ua, 'spider') !== false || stripos($ua, 'crawl') !== false) return '爬虫';
  return '其他';
}

/** 页面路径规整：仅文件名，默认 index.html，白名单外丢弃 */
function st_page_of($p) {
  $p = trim((string)$p);
  if ($p === '' || strpos($p, "\x00") !== false) return 'index.html';
  $p = basename(str_replace('\\', '/', $p));
  if (!preg_match('/^[A-Za-z0-9._-]{1,64}$/', $p)) return 'index.html';
  return $p;
}

/** 来源规整：仅保留主机名（≤120），空 = 直接访问 */
function st_ref_of($r) {
  $r = trim((string)$r);
  if ($r === '') return '直接访问';
  if ($r[0] === '^') $r = substr($r, 1);
  if (strpos($r, '//') !== 0 && preg_match('#^[a-zA-Z][a-zA-Z0-9+.-]*://#', $r) !== 1) $r = '//' . $r;
  $host = parse_url($r, PHP_URL_HOST);
  if (!is_string($host) || $host === '') return '直接访问';
  $host = strtolower($host);
  return (mb_strlen($host) > 120) ? mb_substr($host, 0, 120) : $host;
}

/** 来源完整 URL 规整：仅接受完整 http(s) 链接（去控制字符，≤300 字符），否则 ''（EasySSH vst_ref_full_of 同语义） */
function st_ref_full_of($r) {
  $r = trim((string)$r);
  if ($r === '') return '';
  if ($r[0] === '^') $r = substr($r, 1); // 旧 "^host" 形式无完整 URL
  $r = preg_replace('/[\x00-\x20\x7f]/', '', $r);
  if ($r === '' || preg_match('#^https?://#i', $r) !== 1) return '';
  return (mb_strlen($r) > 300) ? mb_substr($r, 0, 300) : $r;
}

/** 来源分类（R-20261120-005 归因对齐 EasySSH vst_src_classify：github/gitee/search/direct/other） */
function st_src_classify($url) {
  $url = trim((string)$url);
  if ($url === '') return 'direct';
  $host = strtolower((string)parse_url($url, PHP_URL_HOST));
  if ($host === '') return 'other';
  if (strpos($host, 'github.com') !== false) return 'github';
  if (strpos($host, 'gitee.com') !== false) return 'gitee';
  if (preg_match('/(google|bing|baidu|sogou|so\.com|360\.cn|duckduckgo|yandex|yahoo)/', $host) === 1) return 'search';
  return 'other';
}

/** 来源分类 → 中文展示名（后台徽章/卡片/归因区块共用，与前端 SRC_LABEL 同表） */
function st_src_label($src) {
  $map = ['github' => 'GitHub', 'gitee' => 'Gitee', 'search' => '搜索引擎',
          'direct' => '直接访问', 'other' => '其他', 'app-update' => '应用内升级',
          'ip-segment' => 'IP 段匹配', 'unknown' => '未知'];
  return $map[(string)$src] ?? (string)$src;
}

/** 归属地（GEO-20261118 接入）：查询链 ip.263.tw 加密 API → 百度 → ip-api，按 IP 前缀缓存（沿用 EasySSH 同款 263.tw 服务）。返回中文名或 ''。 */
function st_region($ip) {
  if (!function_exists('st_region_name')) {
    $geo = st_geo_probe($ip);
    return is_string($geo) ? $geo : '';
  }
  try { $r = st_region_name((string)$ip); } catch (Throwable $e) { $r = ''; }
  return is_string($r) ? $r : '';
}

define('ST_GEO_CACHE', ST_DATA_DIR . '/stats-geocache.json'); // IP前缀 → 中文地域名 缓存
define('ST_GEO_CACHE_MAX', 20000);
define('ST_GEO_API_TIMEOUT', 3);
define('ST_GEO_PRIMARY', 'https://ip.263.tw/api/v1/');
define('ST_GEO_TOKEN', '33da2b6569fa5bd55a7c1a08a11e0cc91a11b607eb4335f00b438438f399fc90');

function st_geo_cache_load() {
  $j = is_readable(ST_GEO_CACHE) ? json_decode((string)file_get_contents(ST_GEO_CACHE), true) : null;
  return (is_array($j) && isset($j['map']) && is_array($j['map'])) ? $j['map'] : [];
}

function st_geo_probe($ip) {
  $pp = st_ip_prefix($ip);
  if ($pp === null) return '';
  list($family, $prefix, $probe) = $pp;
  $map = st_geo_cache_load();
  if (array_key_exists($prefix, $map)) return (string)$map[$prefix];
  $name = st_geo_query_primary($probe);
  if ($name === '' && $family === 'v4') $name = st_geo_query_baidu($probe);
  if ($name === '') $name = st_geo_query_ipapi($probe);
  if ($name === '') return '';
  if (count($map) >= ST_GEO_CACHE_MAX) $map = array_slice($map, (int)(ST_GEO_CACHE_MAX / 4), null, true);
  $map[$prefix] = $name;
  fb_json_file_save(ST_GEO_CACHE, ['map' => $map]);
  return $name;
}

function st_region_name($ip) { $r = st_geo_probe((string)$ip); return is_string($r) ? $r : ''; }

/** 公网 IP 判定与前缀提取：[family, prefix, probe]；非公网/非法返回 null */
function st_ip_prefix($ip) {
  $ip = (string)$ip;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $seg = explode('.', $ip);
    return ['v4', $seg[0] . '.' . $seg[1], $seg[0] . '.' . $seg[1] . '.1.1'];
  }
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV6 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $h = bin2hex((string)inet_pton($ip));
    $prefix = substr($h, 0, 4) . ':' . substr($h, 4, 4);
    return ['v6', $prefix, $prefix . '::1'];
  }
  return null;
}

function st_ip_mask_label($family, $prefix) {
  return $family === 'v6' ? $prefix . '::/32' : $prefix . '.x.x';
}

function st_geo_query_primary($probe) {
  $ctx = stream_context_create(['http' => [
    'timeout' => ST_GEO_API_TIMEOUT,
    'header'  => "Authorization: Bearer " . ST_GEO_TOKEN . "\r\nContent-Type: application/x-www-form-urlencoded\r\n",
  ]]);
  $resp = @file_get_contents(rtrim(ST_GEO_PRIMARY, '/') . '/ip?q=' . urlencode($probe), false, $ctx);
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  $payload = (is_array($j) && ($j['code'] ?? -1) === 0 && isset($j['data']['payload'])) ? (string)$j['data']['payload'] : '';
  if ($payload === '') return '';
  $resp2 = @file_get_contents(rtrim(ST_GEO_PRIMARY, '/') . '/decrypt', false,
    stream_context_create(['http' => [
      'timeout' => ST_GEO_API_TIMEOUT,
      'method'  => 'POST',
      'header'  => "Authorization: Bearer " . ST_GEO_TOKEN . "\r\nContent-Type: application/x-www-form-urlencoded\r\n",
      'content' => http_build_query(['payload' => $payload]),
    ]]));
  if ($resp2 === false || $resp2 === '') return '';
  $j2 = json_decode($resp2, true);
  $text = (is_array($j2) && ($j2['code'] ?? -1) === 0 && isset($j2['data']['text'])) ? (string)$j2['data']['text'] : '';
  if ($text === '') return '';
  $t = json_decode($text, true);
  if (!is_array($t)) return '';
  $name = '';
  if (!empty($t['raw'])) $name = trim(str_replace(['@', '|'], ' ', (string)$t['raw']));
  if ($name === '') $name = trim((($t['country'] ?? '') . ' ' . ($t['region'] ?? '') . ($t['prov'] ?? '') . ' ' . ($t['city'] ?? '')));
  $name = preg_replace('/\s+[A-Z]{2}$/u', '', $name);
  $name = preg_replace('/^中国\s+/u', '', $name);
  $name = trim($name);
  if ($name === '' || $name === '保留' || mb_strlen($name) > 40) return '';
  return $name;
}

function st_geo_query_baidu($probe) {
  $ctx = stream_context_create(['http' => ['timeout' => ST_GEO_API_TIMEOUT, 'header' => "User-Agent: Mozilla/5.0\r\n"]]);
  $resp = @file_get_contents('https://opendata.baidu.com/api.php?query=' . urlencode($probe) . '&co=&resource_id=6006&oe=utf8', false, $ctx);
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  $loc = (is_array($j) && isset($j['data'][0]['location'])) ? trim((string)$j['data'][0]['location']) : '';
  if ($loc === '' || $loc === '保留' || mb_strlen($loc) > 40) return '';
  return $loc;
}

function st_geo_query_ipapi($probe) {
  $ctx = stream_context_create(['http' => ['timeout' => ST_GEO_API_TIMEOUT, 'header' => "User-Agent: Mozilla/5.0\r\n"]]);
  $resp = @file_get_contents('http://ip-api.com/json/' . urlencode($probe) . '?lang=zh-CN&fields=status,country,regionName,city', false, $ctx);
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  if (!is_array($j) || ($j['status'] ?? '') !== 'success') return '';
  $name = trim((string)($j['country'] ?? '') . ' ' . (string)($j['regionName'] ?? '') . ' ' . (string)($j['city'] ?? ''));
  $name = preg_replace('/^中国\s+/u', '', $name);
  if ($name === '' || mb_strlen($name) > 40) return '';
  return $name;
}

/** 桶计数入库（带上限防灌水） */
function st_bump(&$bucket, $key, $max) {
  if ($key === '' || $key === null) return;
  $bucket[$key] = (int)($bucket[$key] ?? 0) + 1;
  if (count($bucket) > $max) {
    arsort($bucket);
    $bucket = array_slice($bucket, 0, $max, true);
  }
}

/** 遥测字段白名单（与客户端预留约定；地域由服务端按掩码 IP 生成） */
function st_allowed_fields() {
  return ['软件版本', '操作系统', '系统架构', 'CPU', '内存', '磁盘容量', '显卡', '系统语言代码', '客户端类型', '启动次数'];
}

/** 遥测字段清洗：白名单内、值截断、字段数封顶 */
function st_sanitize($data) {
  $allowed = st_allowed_fields();
  $out = [];
  foreach ($allowed as $f) {
    if (!isset($data[$f])) continue;
    $v = trim((string)$data[$f]);
    if ($v === '' || mb_strlen($v) > ST_MAX_VALUE) continue;
    $out[$f] = mb_substr($v, 0, ST_MAX_VALUE);
  }
  return $out;
}

/** 匿名 ID 合法性：16-64 位十六进制 */
function st_valid_anon_id($id) {
  return is_string($id) && preg_match('/^[0-9a-f]{16,64}$/', $id) === 1;
}

/* ---------- 访客打点入库（beacon 写入端，GET/POST 两路共用） ---------- */

function st_visit_record($page, $refRaw) {
  $ip = fb_client_ip();
  $ua = (string)($_SERVER['HTTP_USER_AGENT'] ?? '');
  if (!st_rate_ok(ST_VISIT_RATE_FILE, $ip, ST_VISIT_RATE_PER_HOUR)) return; // 静默丢弃
  // 统计排除页（后台管理页等不记录 PV/UV/IP/流水）
  if (in_array($page, ['admin.html'], true)) return;

  $ref = st_ref_of($refRaw);
  $rf = st_ref_full_of($refRaw);   // 完整来源链接（明细记录详情回显用）
  $uv = st_uv_hash($ip, $ua);
  $now = st_now_ms();
  $d = date('Y-m-d');
  $ipMask = st_ip_mask($ip);
  $uaFam = st_ua_family($ua);

  st_locked_update(ST_VISIT_DB, 'visit', function ($db) use ($d, $now, $uv, $ipMask, $ip, $page, $ref, $rf, $uaFam) {
    if (!isset($db['days'][$d]) || !is_array($db['days'][$d])) {
      // ipsFull = 完整 IP 明细桶（对齐 EasySSH：{完整IP:{c:次数,last:最后时间}}）
      $db['days'][$d] = ['pv' => 0, 'uv' => 0, 'ips' => [], 'ipsFull' => [], 'pages' => [], 'refs' => [], 'uas' => [], 'uvs' => []];
    }
    $day = &$db['days'][$d];
    $day['pv'] = (int)($day['pv'] ?? 0) + 1;
    if (!isset($day['uvs'][$uv])) {
      if (count($day['uvs']) < ST_VISIT_UV_MAX) { $day['uvs'][$uv] = true; $day['uv'] = (int)($day['uv'] ?? 0) + 1; }
    }
    st_bump($day['ips'], $ipMask, ST_VISIT_BUCKET_MAX);
    // 完整 IP 明细（IPv4/IPv6 全量存储，仅运营统计用途；EasySSH track.php 同语义）
    if (!isset($day['ipsFull']) || !is_array($day['ipsFull'])) $day['ipsFull'] = [];
    if (isset($day['ipsFull'][$ip])) {
      $day['ipsFull'][$ip]['c'] = (int)$day['ipsFull'][$ip]['c'] + 1;
      $day['ipsFull'][$ip]['last'] = $now;
    } elseif (count($day['ipsFull']) < ST_VISIT_IPS_FULL_MAX) {
      $day['ipsFull'][$ip] = ['c' => 1, 'last' => $now];
    } else {
      arsort($day['ipsFull']);
      $day['ipsFull'] = array_slice($day['ipsFull'], 0, ST_VISIT_IPS_FULL_MAX, true);
    }
    st_bump($day['pages'], $page, ST_VISIT_BUCKET_MAX);
    st_bump($day['refs'], $ref, ST_VISIT_BUCKET_MAX);
    st_bump($day['uas'], $uaFam, ST_VISIT_BUCKET_MAX);
    unset($day);

    // 超保留期按日记录清理（明细看最近流水）
    $cut = (int)(($now - ST_VISIT_DAYS_KEEP * 24 * 3600 * 1000) / 1000);
    foreach ($db['days'] as $k => $v) {
      if (strtotime((string)$k) !== false && strtotime((string)$k) < $cut) unset($db['days'][$k]);
    }

    // 最近访客流水（最新在前）：
    //   id  = 流水唯一标识（10 位随机 hex，供明细联查/归因反向关联，EasySSH v5.1.6 起）
    //   ip  = 完整 IP（ADMIN-FULLIP：新记录不再掩码；历史掩码记录读端原样兼容显示）
    //   f   = 完整 IP（与 ip 同值，EasySSH 语义对齐）；uv = 按日轮换哈希前 12 位；rf = 完整来源链接
    array_unshift($db['recent'], [
      'id' => bin2hex(random_bytes(5)),
      't' => $now, 'ip' => (string)$ip, 'f' => (string)$ip, 'uv' => substr($uv, 0, 12),
      'ua' => $uaFam, 'p' => $page, 'r' => $ref, 'rf' => $rf,
    ]);
    if (count($db['recent']) > ST_VISIT_RECENT_MAX) $db['recent'] = array_slice($db['recent'], 0, ST_VISIT_RECENT_MAX);
    return $db;
  });
}

/* ---------- ADMIN-FULLPORT：客户端统计合并行与活跃度（api/stats/admin.php 使用） ---------- */

/**
 * 并表视图：遥测设备 + 反馈系统信息伪设备（与 client.php st_all_rows 同口径，
 * 独立命名避免与 client.php 内同名函数冲突）。
 * 每行 = [anonId, firstSeen, lastSeen, reports, src, fields...]，按 lastSeen 倒序。
 */
function st_rows_merged() {
  $db = st_db_load(ST_CLIENT_DB, 'client');
  $fb = fb_db_load();
  $rows = [];
  foreach ($db['clients'] as $id => $c) {
    if (!is_array($c)) continue;
    $rows[] = array_merge(['anonId' => (string)$id, 'firstSeen' => (int)($c['firstSeen'] ?? 0),
      'lastSeen' => (int)($c['lastSeen'] ?? 0), 'reports' => (int)($c['reports'] ?? 1), 'src' => 'telemetry',
      'origin' => null],
      is_array($c['fields'] ?? null) ? $c['fields'] : []);
  }
  foreach ($fb['items'] as $it) {
    if (!is_array($it) || !is_array($it['systemInfo'] ?? null) || !count($it['systemInfo'])) continue;
    $ts = (int)($it['ts'] ?? 0);
    $f = $it['systemInfo'];
    if (($it['appVersion'] ?? '') !== '') $f['软件版本'] = (string)$it['appVersion'];
    $rows[] = array_merge(['anonId' => 'fb:' . (string)($it['id'] ?? ''), 'firstSeen' => $ts,
      'lastSeen' => $ts, 'reports' => 1, 'src' => 'feedback', 'origin' => null], $f);
  }
  usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
  return $rows;
}

/** 设备活跃度：近30天活跃天数 / 连续活跃天数（今天或昨天起算）/ 累计活跃天数（EasySSH st_activity_of 同语义） */
function st_activity_of($db, $anonId) {
  $dates = isset($db['activity'][$anonId]) && is_array($db['activity'][$anonId]) ? $db['activity'][$anonId] : [];
  if (!count($dates)) return ['days30' => 0, 'streak' => 0, 'total' => 0];
  ksort($dates);
  $total = count($dates);
  $now = st_now_ms();
  $in30 = 0;
  foreach ($dates as $k => $v) {
    $t = strtotime((string)$k);
    if ($t !== false && $t * 1000 >= $now - 30 * 24 * 3600 * 1000) $in30++;
  }
  $streak = 0;
  $day = strtotime(date('Y-m-d'));
  if (!isset($dates[date('Y-m-d', $day)])) $day = strtotime('-1 day', $day); // 允许从昨天起算
  while (isset($dates[date('Y-m-d', $day)])) { $streak++; $day = strtotime('-1 day', $day); }
  return ['days30' => $in30, 'streak' => $streak, 'total' => $total];
}
