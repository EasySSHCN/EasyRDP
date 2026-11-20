<?php
/**
 * GET /api/captcha/img.php —— 后台登录图形验证码
 * 响应 JSON：{ ok, id, img:"data:image/png;base64,..." }
 * 验证码存服务端（webroot 之外的 easyrdp-data/），一次性消费，10 分钟过期；
 * 校验在 api/feedback/common.php::fb_captcha_verify（login 动作内调用）。
 */
require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'GET') fb_fail('仅支持 GET', 405);

// 频控为"同 IP 最小间隔 3 秒"(节奏控制):持续速率上限 1200 张/小时,
// 正常刷新无感,拦住高频脚本化拉取(生成 PNG 消耗 CPU)。
$rateKey = hash('sha256', (string)fb_client_ip());
$rateNow = fb_now_ms();
$rate = fb_json_file_load(FB_CAPTCHA_RATE_FILE);
foreach ($rate as $rk => $rv) {
  $lastT = (is_array($rv) && count($rv)) ? (int)end($rv) : 0;
  if (!is_array($rv) || $lastT <= $rateNow - 3600 * 1000) unset($rate[$rk]);
}
if (count($rate) > 5000) $rate = array_slice($rate, -5000, null, true);
$lastMs = isset($rate[$rateKey]) && is_array($rate[$rateKey]) && count($rate[$rateKey]) ? (int)end($rate[$rateKey]) : 0;
$gapMs = 3 * 1000;
if ($rateNow - $lastMs < $gapMs) {
  $rate[$rateKey][] = $rateNow;
  if (count($rate[$rateKey]) > 10) $rate[$rateKey] = array_slice($rate[$rateKey], -10);
  fb_json_file_save(FB_CAPTCHA_RATE_FILE, $rate);
  fb_fail('刷新过于频繁,请 ' . (int)ceil(($gapMs - ($rateNow - $lastMs)) / 1000) . ' 秒后再试', 429);
}
$rate[$rateKey][] = $rateNow;
if (count($rate[$rateKey]) > 10) $rate[$rateKey] = array_slice($rate[$rateKey], -10);
fb_json_file_save(FB_CAPTCHA_RATE_FILE, $rate);

/* ---- 生成随机码：去掉易混淆字符（0/O、1/I/l） ---- */
$alphabet = '23456789abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ';
$n = 5;
$code = '';
for ($i = 0; $i < $n; $i++) $code .= $alphabet[random_int(0, strlen($alphabet) - 1)];
$codeLower = strtolower($code);

/* ---- GD 绘图（深色系，与站点风格一致） ---- */
$w = 150; $h = 46;
$im = imagecreatetruecolor($w, $h);
$bg    = imagecolorallocate($im, 11, 18, 32);   // 深蓝底
$noise = imagecolorallocate($im, 45, 65, 100);
imagefilledrectangle($im, 0, 0, $w, $h, $bg);

for ($i = 0; $i < 4; $i++) {
  imageline($im, random_int(0, $w), random_int(0, $h), random_int(0, $w), random_int(0, $h),
    imagecolorallocate($im, random_int(30, 70), random_int(60, 100), random_int(90, 140)));
}
for ($i = 0; $i < 220; $i++) {
  imagesetpixel($im, random_int(0, $w - 1), random_int(0, $h - 1), $noise);
}
$x = 12;
for ($i = 0; $i < $n; $i++) {
  $ch = $code[$i];
  $color = imagecolorallocate($im, random_int(100, 170), random_int(200, 240), random_int(220, 255));
  imagestring($im, 5, $x, random_int(8, 16), $ch, $color);
  $x += 24 + random_int(-2, 4);
}
imagerectangle($im, 0, 0, $w - 1, $h - 1, imagecolorallocate($im, 45, 70, 110));

ob_start();
imagepng($im);
$png = ob_get_clean();
imagedestroy($im);

/* ---- 存储（服务端一次性消费） ---- */
$now = fb_now_ms();
$db = fb_json_file_load(FB_CAPTCHA_FILE);
foreach ($db as $k => $v) if (!is_array($v) || (int)($v['expires'] ?? 0) <= $now) unset($db[$k]);
if (count($db) > 300) $db = array_slice($db, -300, null, true);
$id = bin2hex(random_bytes(8));
$db[$id] = ['code' => $codeLower, 'expires' => $now + 10 * 60 * 1000];
fb_json_file_save(FB_CAPTCHA_FILE, $db);

fb_json_out([
  'ok' => true,
  'id' => $id,
  'img' => 'data:image/png;base64,' . base64_encode($png),
]);
