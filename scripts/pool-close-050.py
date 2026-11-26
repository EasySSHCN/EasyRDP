#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""v0.5.0 发布收尾：8 条目完成发布回写 + 删 release-request + 人读池/手册同步"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "0.5.0"

NOTES = {
    "R-20261121-001": "随 v0.5.0 发布收口：连接失败自动关闭原生窗口 + map_session_error 五类错误映射（13185/403/504/401/TLS）上线",
    "R-20261121-002": "随 v0.5.0 发布收口：回环桥失败 debug_log 接入 + 就绪日志按 use_tls 输出真实 scheme",
    "R-20261121-003": "随 v0.5.0 发布收口：POLICY-GATE 门禁随本次发布实际执行通过",
    "R-20261122-001": "随 v0.5.0 发布收口：端口敲门客户端兼容全量上线（含 manual/knock.html 手册页 + MANUAL_LINKS 映射）",
    "R-20261122-002": "随 v0.5.0 发布收口：libfko vendor SPA 互通随版发布",
    "R-20261122-003": "随 v0.5.0 发布收口：gateway wss e2e 竞态修复随版（75/75 全过）",
    "R-20261122-004": "随 v0.5.0 发布收口：package-release.ps1 固化短 CARGO_TARGET_DIR（D8050 规避）随版生效",
    "R-20261122-005": "随 v0.5.0 发布收口：Linux AES-256-GCM 密钥保护随版发布（双平台 129 例全绿）",
}

POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] in NOTES:
        it["status"] = "完成发布"
        it["releaseVersion"] = VER
        it.setdefault("statusHistory", []).append({
            "status": "完成发布", "at": now, "note": NOTES[it["id"]]})
# milestone 基线已是 81，复核
assert d["meta"]["milestoneCount"] == 81
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")

# 人读池：追加 8 行（完成后状态）
p1 = os.path.join(ROOT, "docs", "需求变更记录池.md")
s = io.open(p1, encoding="utf-8-sig").read()
rows = """
| R-20261121-001 | 2026-11-21 | 连接失败自动关闭原生窗口 + FreeRDP/网关错误码映射为用户可操作提示（13185 网关传输失败 / 403 策略矩阵拒绝 / 504 后端不可达 / 401 认证过期 / TLS 证书失败）。机器池编号 R-20261121-001 | `crates/easyrdp-rdp/src/gateway_bridge.rs`（map_session_error）、`src-tauri/src/session.rs` | **完成发布** | v0.5.0 |
| R-20261121-002 | 2026-11-21 | 网关回环桥失败日志接入客户端 debug 日志（替换 eprintln!），就绪日志按 use_tls 输出真实 ws/wss scheme。机器池编号 R-20261121-002 | `crates/easyrdp-rdp/src/gateway_bridge.rs` | **完成发布** | v0.5.0 |
| R-20261121-003 | 2026-11-21 | 发布门禁 POLICY-GATE：release.sh 硬校验网关配置模板必须含 [policy] 段与 [[policy.rules]]，缺失即终止发布。机器池编号 R-20261121-003 | `scripts/release.sh`、`scripts/release-check.ps1`、`docs/RELEASE.md` | **完成发布** | v0.5.0 |
| R-20261122-001 | 2026-11-22 | 端口敲门客户端兼容：preConnectCommand 钩子（15s 超时）+ knockd 序列敲门（UDP/TCP、knockd.conf 导入）+ fwknop SPA + 敲门状态可视化与失败分类文案；新增 manual/knock.html 手册页与 MANUAL_LINKS 映射。机器池编号 R-20261122-001 | `crates/easyrdp-core/src/{knock.rs,profile.rs,schema.rs}`、`src-tauri/src/{main.rs,session.rs}`、`src/{index.html,ui/main.js}`、`easyrdp-site/manual/knock.html(新增)` | **完成发布** | v0.5.0 |
| R-20261122-002 | 2026-11-22 | fwknop SPA 互通改造：libfko 2.6.10 vendor 静态编译替换自研加解密，与原生 fwknopd 逐字节互通（WSL 真机正/负向验证通过）；SPA 密钥语义变更见 docs/SPA密钥语义变更说明-20261122.md。机器池编号 R-20261122-002 | `crates/easyrdp-core/vendor/libfko-2.6.10(新增)`、`{build.rs,src/fko.rs(新增),src/knock.rs}`、`examples/spa_send.rs(新增)` | **完成发布** | v0.5.0 |
| R-20261122-003 | 2026-11-22 | gateway wss 自签证书 e2e 并行 KeyMismatch 竞态修复（证书改 RustlsConfig::from_pem 内存加载），默认并行 75/75 全过。机器池编号 R-20261122-003 | `easyrdp-gateway/src/tests_e2e.rs` | **完成发布** | v0.5.0 |
| R-20261122-004 | 2026-11-22 | CI 配置约定：Windows 短 CARGO_TARGET_DIR=C:\\er-target（规避 MSVC D8050）+ Linux gcc vendor libfko 编译验证；本次发布将同约定固化进 package-release.ps1。机器池编号 R-20261122-004 | `.github/workflows/ci.yml`、`easyrdp-client/scripts/package-release.ps1` | **完成发布** | v0.5.0 |
| R-20261122-005 | 2026-11-22 | Linux 密钥保护升级：非 Windows 平台 0600 主密钥文件 + AES-256-GCM（enc: 格式），旧 plain: 数据下次保存自动迁移，双平台 129 例全绿。机器池编号 R-20261122-005 | `crates/easyrdp-core/src/secret.rs`、`Cargo.toml`（aes-gcm） | **完成发布** | v0.5.0 |
"""
if "R-20261121-001" not in s:
    s = s.rstrip("\n") + "\n" + rows.strip("\n") + "\n"
io.open(p1, "w", encoding="utf-8-sig", newline="").write(s)

# 手册修订记录
p2 = os.path.join(ROOT, "docs", "产品开发手册.md")
s = io.open(p2, encoding="utf-8-sig").read()
row = "\n| 2026-11-22 | v1.15 | 产品 | **v0.5.0 minor 里程碑发布**（池累计满 10 条触发，R-20261121-001/002/003 + R-20261122-001~005 同批收口）：连接失败 UX 根治、回环桥可观测性、POLICY-GATE、端口敲门（knockd/fwknop libfko）、Linux 密钥 AES-256-GCM、gateway e2e 竞态修复、CI 短路径约定。三处版本号 0.4.3→0.5.0；milestoneCount=81 基线复核；官网更新记录 v1.10.0；manual/knock.html 新页 + 14 页侧栏互链；中英 changelog 徽章迁移 | \n"
if "| 2026-11-22 | v1.15 |" not in s:
    s = s.rstrip("\n") + row
io.open(p2, "w", encoding="utf-8-sig", newline="").write(s)
print("pool/docs closed for v0.5.0")
