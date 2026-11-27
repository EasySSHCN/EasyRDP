#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.5.1 发布收尾：R-20261122-006 完成发布回写 + 人读池同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.5.1"
ID = "R-20261122-006"
NOTE = "随 v0.5.1 发布收口：客户端五件资产 + Gateway 0.5.1 双平台 Release（GitHub/Gitee）+ 官网同步（8b 15/15 全过）+ 服务器 Linux 网关升级至 0.5.1（/agent/connect /admin/resource /agent/enroll 在线，探针 400/422 拒未授权）+ nginx 放行 ^~ /agent/ 反连路由（/admin/* 保持不暴露）+ OTA 加密升级包 latest.json=0.5.1 线上 200"

POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] == ID:
        it["status"] = "完成发布"
        it["releaseVersion"] = VER
        it.setdefault("statusHistory", []).append({"status": "完成发布", "at": now, "note": NOTE})
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")

p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
if os.path.exists(p1):
    s = io.open(p1, encoding="utf-8-sig").read()
    if "R-20261122-006" in s and "| **完成发布** | v0.5.1 |" not in s:
        # 找到该条目行，更新状态与版本列
        lines = s.splitlines()
        for i, ln in enumerate(lines):
            if ln.startswith("| R-20261122-006 |"):
                # 尾两列为状态与版本
                cells = ln.rsplit("|", 3)
                lines[i] = ln.replace("**已开发未发布**", "**完成发布**").replace("**记录**", "**完成发布**")
                if "v0.5.1" not in lines[i]:
                    lines[i] = lines[i].rstrip() + " v0.5.1 |" if lines[i].rstrip().endswith("|") else lines[i]
        s = "\n".join(lines) + "\n"
        io.open(p1, "w", encoding="utf-8", newline="").write(s)
        print("人读池已更新")
    else:
        print("人读池无需变更或条目缺失（请人工核对）")
else:
    print("人读池文件不存在:", p1)
print("机器池 R-20261122-006 → 完成发布 v0.5.1 回写完成")
