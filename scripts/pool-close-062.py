#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.6.2 发布收尾：R-20261128-001/002 完成发布回写 + 人读池同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.6.2"

NOTES = {
    "R-20261128-001": "随 v0.6.2 发布收口：默认窗口显示大小改为 923x667（实测本机 0.6.1 运行窗口像素，无边框窗口 GetWindowRect 即客户区，DPI 100% 逻辑=物理）；仅影响首次运行/无 winGeom 记忆默认值，已有用户几何由 R-20261119-002 窗口几何记忆保护不受影响",
    "R-20261128-002": "随 v0.6.2 发布收口：标题栏最大化按钮「□」(U+25A1) 字符替换为内联 SVG 空心方框（11x11 viewBox，rect 1,1 9x9 描边 1.6，currentColor，display:block），与最小化/关闭按钮视觉权重一致；关闭/最小化按钮未动",
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
print(f"机器池 {n} 条 → 完成发布 v0.6.2")

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")

# 人读池同步
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
rows = [
    ("R-20261128-001", "2026-11-28", "默认窗口显示大小改为 923x667（对齐用户实际使用习惯）：实测本机 EasyRDP 0.6.1 运行窗口像素（decorations:false 无边框，GetWindowRect 即客户区；DPI 100% 逻辑=物理）；仅影响首次运行/无 winGeom 记忆默认值，已有用户几何由窗口几何记忆（R-20261119-002）保护。随 v0.6.2 发布。机器池编号 R-20261128-001 | `src-tauri/tauri.conf.json` | **完成发布** | v0.6.2"),
    ("R-20261128-002", "2026-11-28", "标题栏最大化按钮图标优化：「□」(U+25A1) 字符字形偏小，替换为内联 SVG 空心方框（11x11 viewBox，1.6 描边，currentColor 自适应主题）；关闭/最小化按钮未动。随 v0.6.2 发布。机器池编号 R-20261128-002 | `src/index.html` | **完成发布** | v0.6.2"),
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
entry = "| 2026-11-28 | v1.15 | 产品 | **v0.6.2 发布**（用户显式下令重新发布）：默认窗口尺寸 923x667（R-20261128-001）+ 标题栏最大化按钮 SVG 空心方框图标（R-20261128-002）；两池同步收口 |\n"
if "v0.6.2 发布" not in s2:
    anchor = s2.find("## 修订记录")
    line_end = s2.find("\n", s2.find("|---|", anchor)) + 1
    s2 = s2[:line_end] + entry + s2[line_end:]
    io.open(p2, "w", encoding="utf-8", newline="").write(s2)
    print("手册修订记录已追加")
print("收口完成")
