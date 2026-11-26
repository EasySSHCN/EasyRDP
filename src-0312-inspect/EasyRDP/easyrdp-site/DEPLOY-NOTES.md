# EasyRDP 官网后台 — 全量保真复刻 EasySSH 版（ADMIN-FULLPORT 2026-11-18）

本次改动仅涉及 **EasyRDP 仓库内文件**，未做任何服务器/部署操作。上一版 4 标签雏形（表格视图）已重写为与 EasySSH 官网后台逐项对齐的卡片式后台（布局/弹窗/分页/交互全对齐）。

## 1. 改动文件

| 文件 | 状态 | 说明 |
|---|---|---|
| `admin.html` | 重写 | 4 标签页后台（EasySSH 2581 行蓝本全量移植）：登录卡片/忘记密码弹窗/邮箱设置弹窗/修改账号弹窗、统计看板（3 卡片明细弹窗 + 9 分布卡 + 提交动态 + 上报设备 + 清空）、反馈（批量删除/系统信息/新增表单/完整分页）、访客统计（9 卡片 + 30 天三柱趋势 + TOP10 + 已产生设备宽幅卡 + 最近访客 + 清空）、官网更新记录（site-updates.json）、标签栏「看板同步 N 秒」与提交动态「自动刷新同步」 |
| `api/stats/admin.php` | 新增 | 客户端统计管理接口（对齐 EasySSH 动作集：summary/list/log/active_list/bucket_list/device/device_log/devices_by_ip/devices_by_region/devices_by_time/visit_link/delete/update/purge），数据源 = 遥测设备 + 反馈系统信息伪设备（`fb:<id>`，只读） |
| `api/stats/visit.php` | 重写 | 写入端补齐 EasySSH 语义字段（流水行 `id/f(完整IP)/rf(完整来源链接)`，按日新增 `ipsFull` 完整 IP 明细桶）；读取端新增 `produced`（已产生设备的访客记录）与 `detail` 的 `uv/ip` 类型 |
| `api/stats/common.php` | 重写 | 保留 client.php/updates.php/visit-beacon.js 依赖的全部旧函数签名；新增 `st_ref_full_of/st_region/st_rows_merged/st_activity_of`，`st_visit_record` 按 EasySSH 语义入库 |
| `api/stats/client.php`、`api/stats/updates.php` | 未改 | 旧接口保留兼容（新 admin.html 不再调用，属遗留只读通道） |
| `api/update/check.php` | 未改 | 轻量更新记录逻辑保留（up_log_event 不变） |
| `api/feedback/*` | 未改 | 协议与 common.php 函数签名不变；EasyRDP 无附件存储，**不实现 attachment.php**（后台附件按钮隐藏，JS 内有注释说明） |
| `assets/js/visit-beacon.js` | 未改 | 现有三级降级（sendBeacon→fetch keepalive→GIF）结构完整，`page/ref` 载荷与新后端兼容 |
| `tools/gen-site-updates.js` | 新增 | EasyRDP 版官网更新数据生成器：从 `changelog.html` 的 release 块解析 → `assets/data/site-updates.json` |
| `assets/data/site-updates.json` | 新增（生成） | 当前版本 0.3.3 / 14 个版本 / 65 条事件；发版后改 changelog.html 再跑 `node tools/gen-site-updates.js` |

## 2. 保留的 EasyRDP 现有特性（逐条已核对）

1. 登录协议不变（login 带 captchaId/captchaCode → 12h token；profile 校验；所有动作 user+token）。
2. 登录记忆块（ADMIN-MEM-20261117）：localStorage `fb_user/fb_token` + 启动 profile 校验自动进后台 + 失效清除 + 退出清除，原样保留。
3. `post()` 兼容签名（单参默认打向 `api/feedback/admin.php`，可双参）原样保留。
4. 站点统一页头（蓝色 logo EasyRDP site-header）与页脚（ADMINUI-20261117 注释块，追加官网版本号 `admin-site-ver`）。
5. 忘记密码流程（fpModal / forgot_request / forgot_reset，绑定邮箱 `admin_email`）完整保留。
6. 配色全部使用 EasyRDP 站点 `:root` 变量（--bg/--bg-card/--border/--text/--text-dim/--accent 蓝色系）；EasySSH 绿色未作主色调，绿色仅「已公开」徽章与趋势图独立 IP 柱等状态语义小范围使用。

## 3. 分页/记忆键对照

| 位置 | 键 | 默认 |
|---|---|---|
| 反馈页大小 | `rdp_admin_page_size`（沿用） | 10 |
| 最近上报设备 | `st_page_size` | 5 |
| 提交动态内嵌/弹窗 | `stlog_page_size` / `stlogm_page_size` | 10 |
| 分布弹窗 / 设备明细弹窗 | `dist_page_size` / `rec_page_size` | 10 |
| 最近访客 / 已产生设备 | `v_page_size`(20) / `vprod_page_size`(10) | — |
| 更新记录 | `upd_page_size` | 5 |
| 标签页 | `admin_tab` | stats |
| 看板同步/动态自动刷新秒数 | `st_full_refresh_sec`(60) / `stlog_refresh_sec`(3) | — |

## 4. 服务器操作清单（上线时）

1. 数据目录已沿用 `/www/wwwroot/easyrdp-data/stats/`（上一版已建则无需再建；新部署 `mkdir -p` + `chown www:www` + `chmod 2775`）。
2. 上传：`admin.html`、`api/stats/{common,admin,visit}.php`、`assets/data/site-updates.json`（可选 `tools/gen-site-updates.js` 仅本机使用）。
3. `keys.php` 无需改动（未新增任何密钥字段；keys.example.php 无变化——未接入外部归属地 API）。
4. `php -l` 全部 API 文件本机 PHP 7.4.33 通过；admin.html 内联 JS 与全部独立 JS `node --check` 通过。
5. 访客统计现在会存储**完整 IP（f 字段）与完整来源链接（rf）**——与 EasySSH 语义对齐（任务要求），如需更严隐私可回退仅掩码（改 `api/stats/common.php` 的 `st_visit_record`）。

## 5. 遗留事项

- 设备来源归因（origin/下载票据/启发式）未移植数据侧：EasyRDP 无下载票据链路；服务端已保留 `origin` 挂接点与 `visit_link` 联查动作，「来源分布」卡片当前恒为「未知」，「已产生设备的访客记录」在有归因数据后自动出现。
- 归属地中文名（st_region）为占位恒空串：未引入 EasySSH 的 ip.263.tw 外呼 API（避免新增外部密钥/依赖）；地域标签当前为掩码 IP 前缀形态。
- 附件查看（attachment.php）未实现：EasyRDP 无附件存储；反馈条目按钮已隐藏并注释，后续接入时参考 EasySSH 版（POST + X-Fb-User/X-Fb-Token 请求头鉴权）。
- 客户端遥测上报（action=report）仍为预留通道，看板数据主要来自反馈 systemInfo 聚合（`fb:<id>` 伪设备，只读不可删改）。
- 旧 `api/stats/client.php`/`updates.php` 保留但已不被后台页面调用。
