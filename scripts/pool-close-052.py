#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.5.2 发布收尾：R-20261123-001 完成发布回写 + 人读池同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.5.2"
ID = "R-20261123-001"
NOTE = "随 v0.5.2 发布收口：客户端 portable sha256=d6ff8807…（FileVersion=0.5.2 换名验证）、GitHub/Gitee v0.5.2 Release 五件、官网 8b 15/15 全过、OTA latest.json=0.5.2 线上 200、服务器 Linux 网关升级 0.5.2（探针 400 在线）；发布链同步固化 BOM-GATE（release.sh 3.7 + push-release.ps1）与 OTA 第 9 步重试降级，杜绝 PS5.1 无 BOM 解析错乱与陈旧二进制打包复发"

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
s = io.open(p1, encoding="utf-8-sig").read()
row = "| R-20261123-001 | 2026-11-23 | OTA 升级死循环双缺陷修复：①make-release-assets.ps1 陈旧二进制打包（v0.5.1 资产实为 0.5.0 内容）→ CARGO_TARGET_DIR 感知 + PE FileVersion 硬校验；②updater.rs 跨卷落位 rename 必败 + 回滚目标错名产生僵尸文件 → 同卷临时名原子落位重试 + 回滚固定原始路径；附带 BOM-GATE 发布门禁（PS5.1 无 BOM 中文脚本解析错乱根治）与 OTA 第 9 步重试降级。随 v0.5.2 发布。机器池编号 R-20261123-001 | `make-release-assets.ps1`、`easyrdp-client/src-tauri/src/updater.rs`、`scripts/release.sh`、`scripts/fix-ps1-bom.ps1(新)` | **完成发布** | v0.5.2 |"
if "R-20261123-001" not in s:
    lines = s.rstrip("\n").split("\n")
    lines.append(row)
    io.open(p1, "w", encoding="utf-8", newline="").write("\n".join(lines) + "\n")
    print("人读池已追加 R-20261123-001")
print("机器池 R-20261123-001 → 完成发布 v0.5.2")
