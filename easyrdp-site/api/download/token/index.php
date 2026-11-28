<?php
/* POST /api/download/token → token 签发（薄路由，实现见 ../token.php；R-20261119-007）
 * 依赖 Apache/Nginx DirectoryIndex 将目录请求映射到本 index.php；
 * 显式扩展名路由 /api/download/token.php 同样可用。 */
require __DIR__ . '/../token.php';
