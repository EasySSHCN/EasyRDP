<?php
/* GET /api/download → 签名 Token 中转下载（薄路由，实现见 get.php；R-20261119-007）
 * 依赖 Apache/Nginx DirectoryIndex 将目录请求映射到本 index.php。 */
require __DIR__ . '/get.php';
