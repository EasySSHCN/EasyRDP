#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""R-20261120-005 开发完成：池状态 待开发→已开发未发布 + 人读池/手册同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")

# 机器池
POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] == "R-20261120-005":
        it["status"] = "已开发未发布"
        it.setdefault("statusHistory", []).append({
            "status": "已开发未发布", "at": now,
            "note": "开发完成（本会话）：服务端归因分级对齐 EasySSH——api/stats/common.php 新增 st_src_classify/st_src_label；client.php 票据 origin 补全 EasySSH 全字段（src/url/visitId/uv/ip/visitTs/downloadAt/attributedAt/confidence）+ 超窗 low 档 + 同 IP 票据扫描 + 方案 B 启发式兜底（保留 ip-segment 为 EasyRDP 特有最低优先兜底）；summary 新增 bySrc 来源分布；st_all_rows 携带 origin；ticket.php 签发补 src/url。admin.html 设备明细弹窗新增来源归因区块 + 归因方式三档文案。客户端软件端（telemetry.rs ticket_id 注入上报）经盘点与 EasySSH tok 机制语义一致，无需改码。线上 E2E 冒烟通过（票据 search 分类/high 置信/attributed 标记），测试数据已清理。php -l 三文件过、admin JS node --check 过。随下个满足发布条件版本发布（官网资产可随任意发布同步，无需等客户端）"})
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

# 人读池
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
old = "| R-20261120-005 | 2026-11-20 | 来源归因对齐 EasySSH（软件端实现 + admin.html 两标签页关联展示全面对齐）：未开发，需先做 EasySSH 基线盘点，保留待下批发布 | 待盘点后定 | 待开发 | — |"
new = "| R-20261120-005 | 2026-11-20 | 来源归因对齐 EasySSH：基线盘点完成（EasySSH st_origin_apply 四级管线/SRC_LABEL/bySrc/明细弹窗 originBlock）；EasyRDP 对齐改造——服务端四级归因（票据精确 high→超窗 low→同 IP 票据扫描→方案 B 启发式→ip-segment 特有兜底）、origin 全字段（src 分类/来源链接/下载时间/归因时间/访问流水联查）、summary bySrc 来源分布、admin 设备明细归因区块与三档归因方式文案；客户端 ticket_id 注入上报与 EasySSH tok 语义一致无需改码。线上 E2E 冒烟通过（百度来源→search 分类→high 票据归因→attributed 标记） | `easyrdp-site/api/stats/{client,common}.php`、`api/download/ticket.php`、`admin.html` | **已开发未发布**（随下个满足发布条件版本发布） | 待发布 |"
if old in s:
    s = s.replace(old, new)
io.open(p1, "w", encoding="utf-8-sig", newline="").write(s)

# 手册修订记录
p2 = os.path.join(ROOT, "docs", "产品开发手册.md")
s = io.open(p2, encoding="utf-8-sig").read()
row = "\n| 2026-11-21 | v1.13 | 产品 | **R-20261120-005 来源归因对齐 EasySSH 开发完成**（EasySSH 基线盘点→EasyRDP 对齐）：①服务端四级归因管线（票据精确 high/超窗 low→同 IP 票据扫描→方案 B 启发式（同完整 IP+firstSeen 前 7 天最近访问，referer 分类）→ip-segment 特有兜底），origin 全字段对齐（src/url/visitId/uv/ip/visitTs/downloadAt/attributedAt/confidence）；②summary 新增 bySrc 来源分布聚合（无 origin 计 unknown）；③st_all_rows 携带 origin 供明细弹窗渲染；④ticket.php 签发补 src（st_src_classify 分类：github/gitee/search/direct/other）与 url；⑤admin.html 设备明细弹窗新增「🔗 来源归因」区块（对齐 EasySSH renderSummary）+ 归因方式三档文案（票据归因（精确/超窗，疑似）/IP+时间启发式（疑似来源）/IP 段匹配（疑似））；⑥客户端软件端经盘点与 EasySSH tok 机制语义一致（telemetry.rs 已注入 ticket_id 随首报携带），无需改码。验证：php -l 三文件、admin JS node --check、线上 E2E 冒烟（百度来源票据→src=search→high 票据归因→attributed 标记）全过，测试数据已清理。状态：已开发未发布，随下个满足发布条件版本发布（官网 PHP 资产已随本次部署线上生效） |\n"
if "| 2026-11-21 | v1.13 |" not in s:
    s = s.rstrip("\n") + row
io.open(p2, "w", encoding="utf-8-sig", newline="").write(s)
print("pool/docs updated for R-20261120-005")
