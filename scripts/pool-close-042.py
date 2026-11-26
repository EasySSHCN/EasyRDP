#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.4.2 发布后需求池回写（UTF-8 无 BOM，池文件绝不外泄/入库）"""
import json, io, shutil, datetime, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
REQ  = os.path.join(ROOT, "reqpool", "release-request.json")

data = json.load(io.open(POOL, encoding="utf-8-sig"))
done = {
    "R-20261120-003": "随 v0.4.2 发布：升级「文件被占用」自动接力闭环上线（self_heal 返回新版路径 + 被占用兜底即时 spawn_detached 重启 + main.rs 接力启动，cargo check 通过，资产 0.4.2 已发布）",
    "R-20261120-004": "随 v0.4.2 发布：遥测频控改造上线（telemetry.rs 新增 last_version 状态，版本变更后每次启动必报一次，同版本保留按日去重，随 v0.4.2 客户端发布生效）",
    "R-20261120-006": "随 v0.4.2 发布：tooltip.js MANUAL_LINKS btn-export → export-json.html，客户端 v0.4.2 构建实际带出",
    "R-20261120-007": "随 v0.4.2 发布：官网全站页脚 data-version 版本号 + curate_site.py 自动替换上线，8b 校验全过",
    "R-20261120-008": "随 v0.4.2 发布：网关 P0 修复上线——v0.4.2 Linux 二进制部署 /opt/easyrdp-gateway，生产 config [auth] allow_placeholder_login=false，公网占位登录实测拒绝；nginx 热修复层核对仅三条精确路由（构建部署详见发布日志）",
    "R-20261120-009": "随 v0.4.2 发布：gw-admin.html 用户服务台 + api/gateway/issue.php（users/grants 自举建表，users_db=/www/wwwroot/easyrdp-data/users.db）+ 网关 /admin/issue 全链上线，线上验证通过",
    "R-20261120-010": "随 v0.4.2 发布：manual/gateway.html 服务状态小节 + faq.html 口径对齐 v0.4.2，官网资产已同步（官网更新记录 v1.8.0）",
}
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
n = 0
for item in data["items"]:
    if item["id"] in done:
        if item.get("status") == "完成发布" and item.get("releaseVersion") == "0.4.2":
            continue  # 幂等：已关闭
        item["status"] = "完成发布"
        item["releaseVersion"] = "0.4.2"
        item.setdefault("statusHistory", []).append({
            "status": "完成发布", "at": now, "note": done[item["id"]]})
        n += 1
print("pool closed:", n)

# release-request.json：移除已完成 ID，保留未开发条目
if os.path.exists(REQ):
    req = json.load(io.open(REQ, encoding="utf-8-sig"))
    req["pendingIds"] = [i for i in req.get("pendingIds", []) if i not in done]
    req["status"] = "requested" if req["pendingIds"] else "done"
    req.setdefault("statusHistory", []).append({
        "at": now,
        "note": "v0.4.2 发布完成：7/8 条关闭；R-20261120-005（来源归因对齐 EasySSH）未开发且需 EasySSH 基线盘点，保留待下批"})
    io.open(REQ, "w", encoding="utf-8", newline="").write(
        json.dumps(req, ensure_ascii=False, indent=4))

bak = POOL + ".bak-042"
if not os.path.exists(bak):
    shutil.copyfile(POOL, bak)
json.dump(data, io.open(POOL, "w", encoding="utf-8", newline=""),
          ensure_ascii=False, indent=4)
print("pool/request written (utf-8 no bom)")
