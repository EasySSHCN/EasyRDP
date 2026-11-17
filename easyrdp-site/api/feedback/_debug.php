<?php
require __DIR__ . '/common.php';
$keys = fb_keys();
$k = fb_derive(base64_decode($keys['master'], true));
fb_json_out([
  'web_master_head' => substr($keys['master'], 0, 12),
  'web_enc_head' => substr(base64_encode($k['enc']), 0, 16),
  'web_mac_head' => substr(base64_encode($k['mac']), 0, 16),
]);
