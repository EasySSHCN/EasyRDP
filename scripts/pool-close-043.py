#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.4.3 发布收尾：R-20261120-005 完成发布 + 删除 release-request + 人读池/手册同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")

POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] == "R-20261120-005":
        it["status"] = "完成发布"
        it["releaseVersion"] = "0.4.3"
        it.setdefault("statusHistory", []).append({
            "status": "完成发布", "at": now,
            "note": "随 v0.4.3 发布收口：归因对齐服务端+后台展示已上线（官网 PHP 随发布同步），客户端随 v0.4.3 构建发布；GitHub/Gitee Release v0.4.3、官网 8b 15/15、OTA latest=0.4.3、GitHub tag push 重试成功"})
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted (pendingIds empty)")

# 人读池：005 行改完成发布
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
old_tail = "**已开发未发布**（随下个满足发布条件版本发布） | 待发布 |"
new_tail = "**完成发布**（客户端随 v0.4.3 构建，服务端/后台已线上生效） | v0.4.3 |"
if old_tail in s:
    s = s.replace(old_tail, new_tail)
io.open(p1, "w", encoding="utf-8-sig", newline="").write(s)

# 手册修订记录
p2 = os.path.join(ROOT, "docs", "产品开发手册.md")
s = io.open(p2, encoding="utf-8-sig").read()
row = "\n| 2026-11-21 | v1.14 | 产品 | **v0.4.3 patch 发布**（用户指示直接发布，R-20261120-005 收口）：客户端无功能变化随版构建发布；服务端归因对齐（四级管线+origin 全字段+bySrc 来源分布+admin 归因区块）已线上生效。三处版本号 0.4.2→0.4.3；GitHub push 首跑认证失败→独立重试脚本一次成功；OTA wslpath 失败按惯例手动补跑（latest=0.4.3 线上 200）；官网 8b 校验 15/15 全过；官网更新记录 v1.9.0；池 005 回写完成发布、release-request.json 删除（pendingIds 清空）；通知邮件已发 |\n"
if "| 2026-11-21 | v1.14 |" not in s:
    s = s.rstrip("\n") + row
io.open(p2, "w", encoding="utf-8-sig", newline="").write(s)
print("pool/docs closed for v0.4.3")
