# CONFLICTS.md — EasyRDP / EasySSH 双项目在 sv 备用服务器上的共存冲突规避设计

sv 服务器同时承载两个项目的完整服务端，所有维度完全隔离，任何一个项目的部署/同步/故障都不得影响另一个。

## 1. 隔离总表

| 维度 | EasyRDP | EasySSH | 隔离手段 |
|---|---|---|---|
| webroot | `/www/wwwroot/easyrdp.cn` | `/www/wwwroot/easyssh.cn` | 独立目录，rsync 目标不同，物理不可能互相覆盖 |
| 运行时数据 | `/www/wwwroot/easyrdp-data` | `/www/wwwroot/data` | 独立目录；且都移出各自 webroot，nginx `deny all` 拦截 |
| server_name | `easyrdp.cn` `www.easyrdp.cn` | `easyssh.cn` `www.easyssh.cn` | 两个独立 vhost 文件，server_name 无交集，nginx 按 Host 精确分发 |
| nginx 配置文件 | `/etc/nginx/conf.d/easyrdp.cn-sv.conf` | `/etc/nginx/conf.d/easyssh.cn-sv.conf` | 文件级隔离，`-t` 校验可单文件定位错误 |
| php-fpm pool | `easyrdp-sv`（sock: `php*-fpm-easyrdp-sv.sock`） | `easyssh-sv`（sock: `php*-fpm-easyssh-sv.sock`） | 独立 pool：进程数/错误日志/open_basedir 互不影响，单站挂死不拖垮另一站 |
| open_basedir | webroot + 自己的数据目录 + /tmp | 同左 | pool 级 php_admin_value，PHP 代码无法读取对方目录 |
| 机密文件 | `easyrdp.cn/api/**/keys.php` | `easyssh.cn/api/feedback/keys.php` | 各自项目内路径，内容互不共享；均为 600 root:www |
| rsync 排除清单 | `EASYRDP_EXCLUDES` | `EASYSSH_EXCLUDES` | 配置文件按项目独立一节，互不引用 |

## 2. 关键设计说明

- **同名机密不共享**：两个项目都有 `api/feedback/keys.php`，但内容（master/admin_hash/token_secret/邮件 key）完全独立、互不复制。轮换时按项目独立操作。
- **rsync 互不覆盖**：`sync-to-sv.sh` 按 `--project` 选择源/目标路径对，easyrdp 的推送永远只写 `/www/wwwroot/easyrdp.cn`，easyssh 永远只写 `/www/wwwroot/easyssh.cn`；`--delete` 白名单也只作用于各自 webroot。
- **域名冲突规避**：两 vhost 均监听 80/443，靠 server_name 区分；sv 端没有通配 `default_server` 兜底到某站点，未知 Host 一律落 nginx 默认页，避免测试时误伤。
- **证书路径隔离**：`/etc/letsencrypt/live/<域名>/` 按域名独立目录，续期互不影响。
- **单向同步语义**：两项目均为「主 → sv」单向推送；sv 只进不出，不存在双主回环导致的覆盖竞争。
- **pool 名与 sock 名约定**：`<项目>-sv` 前缀命名，与主服务器上可能存在的 pool 不冲突；新增项目沿用 `<项目>-sv` 规范。
