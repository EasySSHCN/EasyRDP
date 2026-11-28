# DNS-SWITCH-RUNBOOK — easyrdp.cn / easyssh.cn 切换至 sv 备用服务器手册

> 目标：把两个域名 A 记录切到 sv.trae.show 后，两项目服务端全部功能可用。
> 同步模式【拉取/pull，2026-11-17 用户定版】：**sv.trae.show 定时从两台正式机只读拉取**
> （cron 在 sv 上，`/opt/dsh/sv-standby/sv-pull.sh`，每 6 小时：easyrdp :17 / easyssh :47）。
> sv 绝不回写正式机；正式机侧不存 sv 密钥（密钥面最小化）。
> 旧 push 方案 `sync-to-sv.sh`/`sync-cron-setup.sh`/`sv-sync.conf.example` 已废弃，仅留档。

> ⚠️ 触发条件（用户明令）：只有当用户**同时**①修改域名解析到 sv + ②通知 DSH 时，才视为应急启用；平时 DSH 侧不对接、不部署、不发版到 sv。

## 0. 角色与前提

| 角色 | 主机 | SSH | 说明 |
|---|---|---|---|
| EasyRDP 主 | easyrdp.cn（198.2.231.95） | **22** | webroot /www/wwwroot/easyrdp.cn，数据 /www/wwwroot/easyrdp-data，证书 /www/server/panel/vhost/cert/easyrdp.cn |
| EasySSH 主 | la.trae.show | **4685** | webroot /www/wwwroot/easyssh.cn，数据 /www/wwwroot/data，证书 /www/server/panel/vhost/cert/easyssh.cn |
| 备用 | sv.trae.show（出口 104.233.200.122） | **53422** | 只读拉取方；切换后为只读热备 |

**拉取通道安全（已部署）**：
- sv 专用密钥 `/root/.ssh/id_ed25519_svpull`，公钥以 `restrict,from="104.233.200.122"` 限源写入两台正式机 authorized_keys；
- 两台正式机 firewalld 已为 104.233.200.122 放行各自 SSH 端口（rich rule，permanent）；
- **sv 出口 IP 变化时**需同步更新：① 两台正式机 authorized_keys 的 from= ② 两台正式机 firewalld rich rule。

**家宽动态 IP 白名单（2026-11-18 部署）**：用户家宽出口以 `home.7xm.net` DDNS 为准。两台正式机的 `/root/bin/ddns-whitelist.sh`（cron 每 5 分钟）自动解析该域名并换绑本机管理端口白名单（easyrdp.cn=22/888/28888；la=4685/888/28888）；sv 拉取白名单（104.233.200.122）为固定 IP，不在该脚本管理范围。演练已验证换绑正确；dig 解析失败时脚本跳过不误删。

sv 环境由 `sv-install-web.sh` 一次性安装（nginx + php-fpm + 双 vhost + 双 pool + 权限加固），幂等可重跑。

## 1. 前置检查清单（在 sv 上执行，全部通过才允许切 DNS）

1. **近期增量拉取正常**：`tail -20 /var/log/sv-sync/pull-*.log` 无 FATAL；抽查内容与主机一致（如 `grep latestVersion /www/wwwroot/easyssh.cn/data/site-data.json`、`ls /www/wwwroot/easyrdp-data/`）。
2. `nginx -t` 通过（sv-install-web.sh 已执行）。
3. **本机 Host 头测试**（sv 上）：
   ```bash
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: easyrdp.cn' http://127.0.0.1/   # 200
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: easyssh.cn' http://127.0.0.1/   # 200
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: easyrdp.cn' http://127.0.0.1/downloads/   # 403/200 而非 404
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: easyssh.cn' http://127.0.0.1/api/feedback/keys.php  # 403/0字节（不可泄露）
   curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: easyssh.cn' http://127.0.0.1/data/feedback-db.json  # 404（数据不可公网）
   ```
4. **PHP 冒烟**（sv 上用 php-cli 直接调用，或切 DNS 前用临时 hosts 指到 sv 实测）：
   - easyrdp：`php -r 'require "/www/wwwroot/easyrdp.cn/api/feedback/keys.php";'` 不报错（或对应自检脚本）
   - easyssh：`curl -H 'Host: easyssh.cn' http://127.0.0.1/data/site-data.json` 返回 JSON 且 latestVersion 正确；`/api/update/check` 200
5. **HTTPS**：`openssl s_client -connect 127.0.0.1:443 -servername easyrdp.cn` / `-servername easyssh.cn` 证书链有效（证书来自 pull 同步的宝塔 cert 路径）。

## 2. 切换步骤

1. （可选灰度）先降 DNS TTL 至 60s，提前 1 小时生效。
2. 在 DNS 服务商把 `easyrdp.cn`、`www.easyrdp.cn`、`easyssh.cn`、`www.easyssh.cn` 的 A 记录由主服务器 IP 改为 sv.trae.show 的 IP。
3. **立即停用 sv 上的拉取 cron**（否则下一次拉取会用主服务器旧数据覆盖 sv 热备期产生的运行时数据）：
   ```bash
   crontab -l | grep -v '# sv-pull-managed' | crontab -    # 仅 sv.trae.show 上执行
   ```
   （pull 模式优势：应急只需一处停用，无需登两台正式机。）
4. 观察解析生效（`dig +short easyrdp.cn`），公网实测两个站点首页/下载/升级接口 200。

## 3. sv 热备期间（只读语义）

- 切换后 **sv 为只读热备**：正常读流量可用，但 sv 上产生的运行时数据（新反馈、统计、下载计数）【不会】流回主服务器。
- 若需把 sv 上积累的新数据迁回主服务器，只能以 **sv 数据为只读快照人工比对合并**（见 §4.2），脚本不提供任何自动回写。

## 4. 切回主服务器 / 数据回迁

### 4.1 切回 DNS
1. 主服务器确认服务健康（各自站点 200、PHP 正常）。
2. DNS A 记录改回主服务器 IP。
3. 在 sv 重新注册定时拉取：`/opt/dsh/sv-standby/sv-pull.sh --install-cron`。

### 4.2 sv 新增数据回迁（人工比对，只读快照）
sv 为只读热备，回迁只允许人工操作：
1. 从 sv 拉取只读快照：`rsync -az sv.trae.show:/www/wwwroot/easyssh.cn/ /tmp/sv-snapshot-easyssh/`（数据目录同理）。
2. 与主服务器当前数据做 diff（feedback-db.json / easyrdp-data 逐条比对 ID/时间戳），人工合并新增条目到主服务器。
3. 合并完成后手动触发一次 `--full` 全量拉取，恢复单向基线。

## 5. SSL 证书续期（二选一，必须写死一种流程）

两台正式机证书由 acme.sh/宝塔部署到 `/www/server/panel/vhost/cert/<域名>/`（pull 已同步该路径到 sv 同路径）。切到 sv 后 certbot 在主服务器续期将失败（域名已指向 sv），二选一：

- **方案 A（推荐）：sv 本地续期**
  sv 上运行 acme.sh，用 HTTP-01（`/.well-known/acme-challenge/` 已在 vhost 放行）签发并 `--install-cert` 到同路径，`nginx -s reload`。
  注意：DNS 未切到 sv 前不能在 sv 上续期；切换后由 sv 全权负责续期（可加 cron）。
- **方案 B：主服务器 DNS-01 续期 + 手动同步**
  主服务器 acme.sh 改用 dns-01 挑战，续期不受 A 记录影响，续期后由下次 pull 自动带回（或手动触发一次 `--project <p> --incr`）。

选定一种后就不要混用；本框架默认 `SYNC_LETSENCRYPT=yes`，配合方案 B 使用。

## 6. 注意事项

- **keys.php / 邮件 API key**：`api/feedback/keys.php`（含 master、admin_hash、token_secret、邮件 API key 等）随 pull 加密通道同步到 sv，600 权限。sv 与主共用同一套机密——若日后轮换任一机密，两台主 + sv 三端都要更新，避免站点行为不一致。
- **`.user.ini`**：pull 排除不同步（宝塔专属，chattr +i），sv 端由 install 脚本按 open_basedir 自行管理。
- **数据目录权限**：sv 端数据目录 750、webroot www/nginx 属主由 pull 收尾与 install 脚本保证。
- **单向原则红线（pull 版）**：任何时候都不在 sv 上配置任何**向正式机写入/回传**的任务；正式机侧不得存放 sv 私钥；sv 拉取私钥泄露或出口 IP 变化时立即在两台正式机吊销/更新 authorized_keys。
