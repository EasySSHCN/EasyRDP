/*!
 * EasyRDP 官网访客统计打点（可嵌入所有官网页面）
 * 用法：在页面 </body> 前加 <script src="assets/js/visit-beacon.js" async></script>
 * 行为：页面加载后异步上报一次 page/ref；优先 sendBeacon POST，降级 fetch keepalive，
 *       再降级 1x1 GIF Image。任何失败完全静默，不产生控制台错误、不阻塞页面。
 * 隐私：服务端只存当日 UV 哈希与掩码 IP；admin.html 被服务端排除，不统计。
 */
(function () {
  'use strict';
  try {
    var page = location.pathname.split('/').pop() || 'index.html';
    var payload = JSON.stringify({ page: page, ref: document.referrer || '', ts: Date.now() });
    /* 以页面所在目录解析 api/stats/visit.php */
    var url = new URL('api/stats/visit.php', location.href).href;

    var sent = false;
    var fallbackGif = function () {
      if (sent) return; sent = true;
      try {
        var img = new Image();
        img.src = url + '?p=' + encodeURIComponent(page) +
          '&r=' + encodeURIComponent(document.referrer || '') + '&t=' + Date.now();
      } catch (e) { /* 静默 */ }
    };
    try {
      if (navigator.sendBeacon && navigator.sendBeacon(url, new Blob([payload], { type: 'application/json' }))) {
        sent = true; return;
      }
    } catch (e) { /* 静默降级 */ }
    try {
      fetch(url, { method: 'POST', keepalive: true, body: payload,
        headers: { 'Content-Type': 'application/json' } })
        .then(function () { sent = true; }).catch(fallbackGif);
      return;
    } catch (e) { /* 静默降级 */ }
    fallbackGif();
  } catch (e) { /* 全静默 */ }
})();
