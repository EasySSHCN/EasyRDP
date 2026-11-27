#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.6.0 里程碑发布收尾：R-20261126-001~008 完成发布回写 + meta.milestoneCount=96 + 人读池同步 + 删除发布请求"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.6.0"

NOTES = {
    "R-20261126-001": "随 v0.6.0 发布收口：原生窗口 WM_PAINT 重绘几何与 present 直绘完全对齐（画面 TITLE_BAR_H 起绘+高度扣标题栏，根治拖动时顶部错位/底部旧像素残留）；非整数缩放下 StretchDIBits 统一 COLORONCOLOR 拉伸模式（present 与 WM_PAINT 两路同口径）",
    "R-20261126-002": "随 v0.6.0 发布收口（核实性闭环）：.rdp 批量导入功能已在 v0.2.1 之后版本提供（R-20261118-008 导入JSON 向导：多文件混选/预览勾选/目标分组/重复高亮/三种去重策略），反馈用户基于 v0.2.1 旧版；v0.6.0 发布说明已告知升级即可使用，无新开发",
    "R-20261126-003": "随 v0.6.0 发布收口：CLIPRDR 通道重连自愈——ChannelConnected 重连分支重建上下文（覆盖旧指针防悬垂）、检测重连留痕日志、缓存文本待 MonitorReady 后自动重新公告，断线重连后复制粘贴无需重开会话",
    "R-20261126-004": "随 v0.6.0 发布收口（核实性闭环）：连接分组（R-20261118-007：3 层树+8 色标记+拖拽归类）与多会话并行（R-20261116-007 会话条+快捷断开）均已在 v0.2.1 之后版本提供；「多标签页」单窗口形态登记为后续版本需求评估项",
    "R-20261126-005": "随 v0.6.0 发布收口：远端→本地文本写入移出 FreeRDP 事件线程（异步落盘，粘贴不再卡画面）；UTF-16 解码严格校验+乱码归因日志（非法代理对/奇数字节长度明确责任在远端）",
    "R-20261126-006": "随 v0.6.0 发布收口：原生窗口标题栏新增「截图」按钮（FRAME_STORE 最新帧→PNG→图片\\EasyRDP）与「N fps」帧率显示；画布路径工具栏新增 📷 截图按钮（save_canvas_screenshot）；四语言 i18n 补 shotSaved；manual/screenshot.html 新页",
    "R-20261126-007": "随 v0.6.0 发布收口：全量 spawn 点审计——补齐 3 处 CREATE_NO_WINDOW（easyrdp-core git_sync.rs RealGitRunner.run + git_available 启动探测、src-tauri session.rs run_pre_connect_command 的 cmd.exe），GUI 下不再闪现黑色控制台窗口（feedback/shortcuts/taskbar_pin 原有防线不变）",
    "R-20261126-008": "随 v0.6.0 发布收口：连接中心新增「灾备包」按钮 + export_dr_package 命令——连接+分组+模板+设置单文件 JSON 备份（密码保持本机 DPAPI 加密、跨机需重填凭据）；manual/dr-package.html 新页",
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
# 里程碑基线更新：本轮累计条目总数（开始下一轮 10 条累计）
d.setdefault("meta", {})["milestoneCount"] = 96
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)
print(f"机器池 {n} 条 → 完成发布 v0.6.0；meta.milestoneCount=96")

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")

# 人读池同步
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
rows = [
    ("R-20261126-001", "2026-11-26", "拖动残影根治：原生窗口 WM_PAINT 重绘几何与 present 直绘对齐（画面从标题栏下方起绘、高度扣除标题栏），消除拖动/改尺寸时顶部错位与底部旧像素残留；非整数缩放比下 StretchDIBits 统一 COLORONCOLOR 拉伸模式。随 v0.6.0 发布。机器池编号 R-20261126-001 | `src-tauri/src/native_view.rs` | **完成发布** | v0.6.0"),
    ("R-20261126-002", "2026-11-26", ".rdp 批量导入核实性闭环：功能已在 v0.2.1 之后版本提供（导入JSON 向导：多文件混选/预览勾选/目标分组/重复高亮/跳过·覆盖·保留三种去重策略），反馈用户基于 v0.2.1 旧版；v0.6.0 发布说明告知升级即可使用。机器池编号 R-20261126-002 | `crates/easyrdp-core/src/batch_import.rs`（既有） | **完成发布** | v0.6.0"),
    ("R-20261126-003", "2026-11-26", "断线重连剪贴板自动恢复：CLIPRDR 虚拟通道重连自愈——ChannelConnected 重连分支重建上下文（覆盖旧指针防悬垂）、重连留痕日志、缓存文本 MonitorReady 后自动重新公告，复制粘贴无需重开会话。随 v0.6.0 发布。机器池编号 R-20261126-003 | `crates/easyrdp-rdp/src/ffi.rs` | **完成发布** | v0.6.0"),
    ("R-20261126-004", "2026-11-26", "分组与多会话核实性闭环：连接分组（3 层树+8 色+拖拽）与多会话并行（会话条+快捷断开）均已在 v0.2.1 之后版本提供；「多标签页」单窗口形态登记为后续评估项。随 v0.6.0 发布说明收口。机器池编号 R-20261126-004 | —（既有能力） | **完成发布** | v0.6.0"),
    ("R-20261126-005", "2026-11-26", "剪贴板大文本优化：远端→本地文本写入移出 FreeRDP 事件线程（异步落盘，粘贴不再卡画面）；UTF-16 解码严格校验+乱码归因日志（非法代理对/奇数字节长度明确责任在远端）。随 v0.6.0 发布。机器池编号 R-20261126-005 | `crates/easyrdp-rdp/src/ffi.rs` | **完成发布** | v0.6.0"),
    ("R-20261126-006", "2026-11-26", "会话帧率显示+一键截图：原生窗口标题栏新增「截图」按钮与「N fps」显示（FPS_STATS 秒窗统计）；画布路径工具栏新增 📷 截图按钮（save_canvas_screenshot）；两路均存「图片\\EasyRDP」远端原始分辨率 PNG；四语言词典补 shotSaved；manual/screenshot.html 新页。随 v0.6.0 发布。机器池编号 R-20261126-006 | `src-tauri/src/native_view.rs`、`src-tauri/src/main.rs`、`src/ui/rdp-view.js`、`src/index.html`、i18n 词典 | **完成发布** | v0.6.0"),
    ("R-20261126-007", "2026-11-26", "启动黑窗根治：全量 spawn 点审计补齐 3 处 CREATE_NO_WINDOW——git_sync.rs RealGitRunner/git_available（启动探测与同步闪窗）、session.rs run_pre_connect_command cmd.exe（连接前命令闪窗）；既有 feedback/shortcuts/taskbar_pin 防线不变。随 v0.6.0 发布。机器池编号 R-20261126-007 | `crates/easyrdp-core/src/git_sync.rs`、`src-tauri/src/session.rs` | **完成发布** | v0.6.0"),
    ("R-20261126-008", "2026-11-26", "灾备包导出入口：连接中心新增「灾备包」按钮 + export_dr_package 命令——连接+分组+模板+设置单文件 JSON 备份（密码保持本机 DPAPI 加密、跨机恢复需重填凭据）；manual/dr-package.html 新页+全手册 toc/index 条目。随 v0.6.0 发布。机器池编号 R-20261126-008 | `src-tauri/src/main.rs`、`src/ui/main.js`、`src/index.html` | **完成发布** | v0.6.0"),
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
entry = "| 2026-11-27 | v1.14 | 产品 | **v0.6.0 里程碑发布**（2026-11-26 官网反馈批次 8 条全处理）：拖动残影根治、.rdp 批量导入核实性收口、剪贴板重连自愈、分组/多会话核实性收口、剪贴板大文本优化、帧率显示+一键截图、启动黑窗根治、灾备包导出入口；池 meta.milestoneCount 更新为 96（下一轮 10 条累计基线） |\n"
if "v0.6.0 里程碑发布" not in s2:
    anchor = s2.find("## 修订记录")
    line_end = s2.find("\n", s2.find("|---|", anchor)) + 1
    s2 = s2[:line_end] + entry + s2[line_end:]
    io.open(p2, "w", encoding="utf-8", newline="").write(s2)
    print("手册修订记录已追加")
print("收口完成")
