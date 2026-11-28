#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.6.3 发布收尾：R-20261128-003 完成发布回写 + 人读池同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.6.3"

NOTES = {
    "R-20261128-003": "随 v0.6.3 发布收口：标题栏最小化/最大化/关闭三按钮 SVG 图标描边统一为 1.5（11x11 viewBox，line/rect/双对角线，round cap，currentColor 自适应主题），消除此前字符字形与 SVG 方框粗细不一致；style.css .tb-btn 改 flex 居中，最大化按钮视觉居中",
}

POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
n = 0
for it in d["items"]:
    if it["id"] in NOTES:
        it["status"] = "完成发布"
        it["releaseVersion"] = VER
        it.setdefault("statusHistory", []).append({"status": "完成发布", "at": now, "note": NOTES[it["id"]]})
        n += 1
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)
print(f"机器池 {n} 条 → 完成发布 v0.6.3")

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")

# 人读池同步
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
rows = [
    ("R-20261128-003", "2026-11-28", "标题栏三按钮粗细统一：最小化/最大化/关闭三按钮图标全部 SVG 化（11x11 viewBox：line/rect/双对角线，stroke 1.5，round cap，currentColor 自适应主题），消除字符字形（─/✕）与 SVG 方框粗细不一致；.tb-btn 改 flex 居中，最大化按钮视觉居中。随 v0.6.3 发布。机器池编号 R-20261128-003 | `src/index.html` / `src/style.css` | **完成发布** | v0.6.3"),
]
added = 0
for rid, date, body in rows:
    if rid not in s:
        s = s.rstrip("\n") + f"\n| {rid} | {date} | {body}\n"
        added += 1
io.open(p1, "w", encoding="utf-8", newline="").write(s)
print(f"人读池追加 {added} 行")

# 手册修订记录
p2 = os.path.join(ROOT, "docs", "产品开发手册.md")
s2 = io.open(p2, encoding="utf-8-sig").read()
entry = "| 2026-11-28 | v1.16 | 产品 | **v0.6.3 发布**：标题栏三按钮粗细统一为 SVG 1.5 描边 + 最大化居中（R-20261128-003）；两池同步收口 |\n"
if "v0.6.3 发布" not in s2:
    anchor = s2.find("## 修订记录")
    line_end = s2.find("\n", s2.find("|---|", anchor)) + 1
    s2 = s2[:line_end] + entry + s2[line_end:]
    io.open(p2, "w", encoding="utf-8", newline="").write(s2)
    print("手册修订记录已追加")
print("收口完成")
