# EasyRDP

跨平台开源远程桌面客户端（Windows / macOS / Linux）· Rust + Tauri 2 + FreeRDP · MIT 协议

**官方网站：https://EasyRDP.CN** · [GitHub](https://github.com/EasySSHCN/EasyRDP) · [Gitee](https://gitee.com/easyssh/EasyRDP)

## 目录结构

| 目录 | 说明 |
|---|---|
| `easyrdp-client/` | 跨平台客户端（Rust workspace：easyrdp-core + src-tauri 壳） |
| `easyrdp-gateway/` | 云网关服务（axum + tokio：认证/隧道/审计/策略） |
| `easyrdp-site/` | 官网静态站（已部署 https://easyrdp.cn ） |
| `docs/` | 产品 PRD 与架构文档 |

## 快速开始

```bash
# 客户端（需 Rust stable 工具链）
cd easyrdp-client
cargo test -p easyrdp-core     # 核心库单元测试
cargo tauri dev                # 桌面端调试运行

# 网关
cd easyrdp-gateway
cargo test                     # 8 个单元测试
cargo run                      # 默认监听 0.0.0.0:8443
```

## 版本更新历史记录

（本 README 为版本历史权威源，CHANGELOG.md 由本节生成）

### v0.1.0（正式版候选，草稿）

- 变更：版本号由 0.1.0-alpha 升级为 0.1.0（正式版候选），客户端各 crate 版本统一对齐
- 新增：原生 FreeRDP2 FFI 实现（feature gate 后置启用，画面引擎接入的基础）
- 新增：发布流水线（GitHub Actions）：push `v*` 标签自动触发 Windows / macOS / Linux 三平台构建并附 GitHub Release + 全量 SHA256SUMS.txt
- 新增：便携版产物策略 — Windows portable.exe / win32-x64-portable.zip、macOS dmg、Linux AppImage；绿色免安装，下载解压（或直接双击）即用，不提供 MSI/NSIS/deb/rpm 安装器
- 修复：CI 修复若干 — workflow `secrets` 上下文误用于 step 条件导致 startup failure、`taiki-e/install-action@tauri-cli` 解析失效（改用 npm 官方预编译 @tauri-apps/cli）、补齐 `src-tauri/icons/icon.png`（Linux/macOS codegen 必需）

### v0.1.0-alpha (2026-11-11)

- 新增：连接中心 UI 原型与客户端 MVP 骨架（Tauri 2 + Rust workspace）
- 新增：easyrdp-core — SessionProfile 模型（serde），含校验逻辑
- 新增：.rdp 文件导入/导出（UTF-16LE/UTF-8 自动识别，key:type:value 解析，往返一致性测试，共 19 个单测）
- 新增：easyrdp-gateway — OIDC→短期 JWT 认证、WebSocket 隧道骨架（审计打点）、环形缓冲审计日志、重定向策略矩阵（8 个单测）
- 新增：官网 https://EasyRDP.CN 上线（首页/功能/下载/更新日志/文档/FAQ/贡献/协议/安全 9 页）
- 新增：产品 PRD 与 UI 原型（基于 mstsc.exe 逆向分析的架构方案）
