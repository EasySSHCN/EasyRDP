# EasyRDP 灾备包（双角色版）— 总览

打包时间：2026-11-18 ｜ 源码版本：v0.3.3 ｜ 结构：按设备角色分目录，各带一键安装脚本

## ⚠️ 机密提示（先读）
本包为**完整复刻包**，内含加密凭据库与服务器明文机密（keys.php）：
- 请整包存放于加密卷/受控存储，**严禁提交仓库、发网盘/邮箱原文**；
- 遗失即视为官网服务器与代码仓库凭据全部泄漏，需立即轮换
  （GitHub/Gitee 令牌、OTA 密钥、keys.php 全部字段、服务器 SSH）。

## 目录结构

```
├── dev-machine/                    ← 【开发机角色】（Windows 10/11 或 Linux）
│   ├── EasyRDP/                    ← 项目全量源码目录（含 secrets-vault 加密库/reqpool/dist 历史产物/scripts 定时任务脚本）
│   ├── tools/                      ← vcpkg 快照 + FreeRDP 2.11.2 源码树 + pkgconf + crate 缓存 + overlay-ports
│   ├── install-windows.ps1         ← Windows 一键安装（工具链/VS BuildTools/WSL2/WebView2/DSH 开发角色/定时任务）
│   ├── install-linux.sh            ← Linux 一键安装（Rust/Tauri 依赖/FreeRDP2-dev/pwsh/cron/DSH 开发角色）
│   └── RESTORE-DEV.md              ← 开发机角色恢复指南（必读）
├── remote-server/                  ← 【远程服务器角色】（easyrdp.cn，Debian/Ubuntu）
│   ├── wwwroot/easyrdp.cn/         ← /www/wwwroot/easyrdp.cn 全量快照（页面/api（含 keys.php）/downloads/.well-known）
│   ├── easyrdp-data/               ← 反馈数据目录（/www/wwwroot/easyrdp-data）
│   ├── keys.php                    ← 【明文机密】反馈后台与接口密钥（与包内 api/feedback/keys.php 同源）
│   ├── sys-config/                 ← 原服务器 nginx 站点配置 + php-fpm 池配置快照
│   ├── install.sh                  ← 服务器一键安装（nginx+PHP-FPM+站点+系统配置+权限+缓存策略）
│   └── SERVER-NOTES.md             ← 服务器角色说明（域名/SSL/凭据/验证清单）
└── README.md                       ← 本文档
```

## 恢复顺序建议

1. **先恢复远程服务器角色**（独立设备）：`remote-server/install.sh`（sudo bash），
   域名 easyrdp.cn 解析到新机后配 HTTPS（certbot）。
2. **再恢复开发机角色**：
   - Windows：管理员 PowerShell 执行 `install-windows.ps1`（含 DSH 开发角色与定时任务）；
   - Linux 开发机：`sudo bash install-linux.sh`（含 DSH 安装指引与 cron）。
3. **联调验证**：开发机 release.sh 发一版测试构建 → 官网 12 项校验 → OTA 验签。

## 角色职责边界

| 角色 | 职责 | 定时任务 |
|---|---|---|
| 开发机 | 源码开发/构建/发布（GitHub+Gitee+官网+OTA）、需求池流水线、反馈采集与复审 | 反馈采集 08:00/20:00；发布巡检 04:00（DSH 提示型 + Windows 计划任务/cron 直执行型） |
| 远程服务器 | easyrdp.cn 静态站 + PHP API（反馈提交/管理员/验证码、统计、更新检测信封、OTA 加密包分发） | 无（按需） |

## 关键机密索引（均在本包内/服务器上，切勿外传）

| 机密 | 位置 | 说明 |
|---|---|---|
| GitHub/Gitee 令牌 | dev-machine → EasyRDP/ 内 `secrets-vault/creds.enc`+`key`（加密） | release.sh 自动解密 |
| 官网服务器 SSH/路径 | 同上 `site-server.enc`+`key`（加密） | 发布同步/OTA 上传 |
| OTA Ed25519 私钥 | 同上 `ota-keys.enc`+`key`（加密）；公钥 `ota-pub.b64` | OTA 签名 |
| 反馈接口 master/token_secret/admin_hash | remote-server → `keys.php` 及 `wwwroot/easyrdp.cn/api/feedback/keys.php`（明文） | 官网后台登录/接口 |
| 更新接口密钥 | remote-server → `wwwroot/easyrdp.cn/api/update/keys.php`（明文） | 更新检测信封 |
| 官网服务器 SSH 私钥 | **不在本包**（原机 P:\Program Files (x86)\EasySSH\keys） | 需单独迁移 |

详细操作步骤见各角色目录内指南。
