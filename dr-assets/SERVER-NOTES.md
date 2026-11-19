# 远程服务器角色说明（easyrdp.cn）

原服务器：198.2.231.95（Debian 系，宝塔目录布局 /www/wwwroot，PHP-FPM + nginx）
角色职责：静态官网 + PHP API（反馈提交/管理员/验证码、统计 client/visit、
更新检测信封 check.php/latest.json、OTA 加密包分发）

## 一键安装

```bash
sudo bash install.sh                    # 默认 webroot=/www/wwwroot，域名 easyrdp.cn
# sudo bash install.sh --webroot=/www/wwwroot --server-name=easyrdp.cn
```

脚本动作：apt 装 nginx/PHP-FPM/扩展 → 部署站点全源（页面/api（含 keys.php）/
downloads/.well-known）→ 反馈数据目录还原 → 部署 sys-config 系统配置快照
（nginx 站点配置 + php-fpm 池）→ nginx 站点配置（HTML no-cache + PHP FastCGI）→
权限收紧（keys.php 600）→ 自检。幂等可重跑。

## 上线核对清单

1. 域名 easyrdp.cn A 记录指向新机；`curl -I http://localhost/download.html` 200；
2. `php -v` 与原版本一致（原机 PHP 8.x）；`systemctl status php*-fpm` 运行中；
3. `curl http://localhost/api/update/latest.json` 返回加密信封（mac ok 由客户端验）；
4. 软件端「检查更新」与官网反馈提交全流程回归；
5. HTTPS：`certbot --nginx -d easyrdp.cn`（域名解析就绪后；.well-known/acme 已随包就位）；
6. 后台登录：https://easyrdp.cn/admin.html（凭据由项目持有者保管；
   忘记时按 keys.php 的 admin_hash 重置，方法见 docs/产品开发手册 v1.6 记录）。

## 注意

- `keys.php` 为明文机密：权限 600 root:root，绝不入 git/发布包；
  包内根目录 keys.php 与 `wwwroot/easyrdp.cn/api/feedback/keys.php` 同源备份；
- `downloads/` 内含全部历史产物（含 OTA 加密包 `.enc`）——删文件前先确认
  历史版本直链（download.html 历史版本表格）不再被引用；
- `sys-config/` 为原服务器（宝塔 sv 环境）配置快照：install.sh 按标准
  Debian/Ubuntu 布局落地；若新机沿用宝塔布局，请对照快照手工调整；
- 服务器无定时任务；反馈采集/发布巡检在开发机角色。
