#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.4.2 发布后：人读池 docs/需求变更记录池.md 追加条目 + docs/产品开发手册.md 修订记录"""
import io, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

pool_rows = """
| R-20261120-003 | 2026-11-20 | 在线升级「文件被占用」兜底路径断链修复：self_heal() 返回新版路径 + 被占用兜底写标记后立即 spawn_detached 重启 + main.rs 接力启动新版（零人工二次点击），占用重试期 emit 进度文案。机器池编号 R-20261120-003 | `src-tauri/src/updater.rs`、`src-tauri/src/main.rs` | **完成发布** | v0.4.2 |
| R-20261120-004 | 2026-11-20 | 遥测上报频控改造：软件更新新版后每次打开必上报一次动态（telemetry.json 新增 last_version，版本变更放行）；同版本重复启动保留按日去重。机器池编号 R-20261120-004 | `src-tauri/src/telemetry.rs` | **完成发布** | v0.4.2 |
| R-20261120-006 | 2026-11-20 | 「导出」按钮悬停手册链接错指 export.html → export-json.html（一行映射修正）。机器池编号 R-20261120-006 | `src/ui/tooltip.js` | **完成发布** | v0.4.2 |
| R-20261120-007 | 2026-11-20 | 官网全站页脚展示版本号并随版本发布自动同步（data-version 占位 + curate_site.py 旧→新自动替换，admin 后台页除外）。机器池编号 R-20261120-007 | `easyrdp-site/*`、`scripts/curate_site.py` | **完成发布** | v0.4.2 |
| R-20261120-008 | 2026-11-20 | P0 网关占位登录公网暴露封堵：allow_placeholder_login 总闸（生产 false）+ nginx 热修复层核对。机器池编号 R-20261120-008 | `easyrdp-gateway/src/{config,auth}.rs`、服务器 `/opt/easyrdp-gateway`、nginx gateway-proxy.conf | **完成发布** | v0.4.2 |
| R-20261120-009 | 2026-11-20 | 用户侧云网关 Web 管理台：gw-admin.html（注册/登录/我的资源/自助签发 3600s token/吊销）+ 网关 POST /admin/issue（X-Admin-Key，TTL 钳制）+ api/gateway/issue.php 回环签发（users/grants 自举建表，/www/wwwroot/easyrdp-data/users.db）。线上注册/登录冒烟验证通过。机器池编号 R-20261120-009 | `easyrdp-gateway/src/audit.rs`、`easyrdp-site/gw-admin.html`、`easyrdp-site/api/gateway/issue.php` | **完成发布** | v0.4.2 |
| R-20261120-010 | 2026-11-20 | 手册与 FAQ 云网关口径矛盾修正：gateway.html 补「云网关服务状态」与 gw-admin.html 入口；faq.html 正文+JSON-LD 对齐 v0.4.2。机器池编号 R-20261120-010 | `easyrdp-site/manual/gateway.html`、`easyrdp-site/faq.html` | **完成发布** | v0.4.2 |
| R-20261120-005 | 2026-11-20 | 来源归因对齐 EasySSH（软件端实现 + admin.html 两标签页关联展示全面对齐）：未开发，需先做 EasySSH 基线盘点，保留待下批发布 | 待盘点后定 | 待开发 | — |
"""

handbook_row = """
| 2026-11-21 | v1.12 | 产品 | **v0.4.2 patch 发布**（release-request 驱动 8 条，巡检+发布一气呵成）：①升级「文件被占用」自动接力闭环（R-20261120-003，self_heal 返回新版路径+兜底 spawn_detached 重启+main.rs 接力，零二次点击）；②遥测新版必报（R-20261120-004，telemetry.rs last_version 频控改造，会话内开发）；③「导出」手册链接改指 export-json.html（R-20261120-006）；④官网全站页脚版本号自动同步（R-20261120-007，8b 全过）；⑤网关 P0 占位登录封堵（R-20261120-008，allow_placeholder_login=false 生产生效，公网实测 403）；⑥用户侧云网关服务台 gw-admin.html + /admin/issue + issue.php 全链上线（R-20261120-009，注册/登录冒烟验证通过，users.db 自举 /www/wwwroot/easyrdp-data）；⑦手册/FAQ 云网关口径修正（R-20261120-010，官网更新记录 v1.8.0）。发布实录与流程新知：三处版本号 0.4.1→0.4.2；GitHub push 首跑 GnuTLS 失败重试成功；OTA 第 9 步 wslpath 失败按惯例手动补跑（latest.json 200）；**重大新知①：PowerShell 5.1 Get-Content -Raw/Set-Content 重编码会把 UTF-8 文件中中文注释行尾换行吞并（GBK 双字节吞 0x0A）且加 BOM——本次损坏两个 Cargo.toml 与 tauri.conf.json，从 v0.4.1 源码包提取原件恢复；版本号改动一律改用编辑工具或 python**；**重大新知②：服务器网关为 Linux ELF（CentOS 9 + 服务器端 cargo 构建部署），win-x64 zip 二进制不可直接部署 /opt——本次误装 PE 致服务 crash-loop，已从 Gitee v0.4.2 tag 在服务器端 cargo build --release 恢复并建立该部署路径惯例**；官网 8b 校验 15/15 全过；池 7 条回写完成发布；R-20261120-005 未开发需 EasySSH 基线盘点保留待下批；通知邮件已发 |
"""

p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
if "R-20261120-003" not in s or "| R-20261120-003 |" not in s:
    s = s.rstrip("\n") + "\n" + pool_rows.strip("\n") + "\n"
io.open(p1, "w", encoding="utf-8-sig", newline="").write(s)

p2 = os.path.join(ROOT, "docs", "产品开发手册.md")
s = io.open(p2, encoding="utf-8-sig").read()
if "| 2026-11-21 | v1.12 |" not in s:
    s = s.rstrip("\n") + "\n" + handbook_row.strip("\n") + "\n"
io.open(p2, "w", encoding="utf-8-sig", newline="").write(s)
print("docs updated")
