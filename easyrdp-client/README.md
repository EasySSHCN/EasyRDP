# EasyRDP

跨平台远程桌面客户端（MVP 骨架），基于 **Rust + Tauri 2 + FreeRDP**，目标平台 Windows / macOS / Linux。

## 架构

```
┌───────────────────────────────────────────────┐
│                UI 壳 (Tauri 2)                │
│   WebView2 / WKWebView / WebKitGTK + HTML/JS  │
└──────────────────┬────────────────────────────┘
                   │ Tauri IPC (invoke)
┌──────────────────▼────────────────────────────┐
│           会话引擎 (src-tauri / Rust)          │
│  import_rdp / list_profiles / connect (规划)   │
└───────┬───────────────────────────┬───────────┘
        │                           │
┌───────▼──────────────┐   ┌────────▼──────────┐
│  Profile 配置模块     │   │  网关集成 (规划)    │
│  easyrdp-core        │   │  OTP Cookie / SSO  │
│  .rdp 导入/导出/校验  │   │  RD Gateway 隧道   │
└──────────────────────┘   └───────────────────┘
        │
┌───────▼───────────────────────────────────────┐
│  RDP 协议层 (规划: crates/easyrdp-rdp)         │
│  FreeRDP (libfreerdp) FFI 封装                 │
└───────────────────────────────────────────────┘
```

## 技术栈

| 层 | 技术 |
|---|---|
| UI 壳 | Tauri 2, WebView2 (Windows) / WKWebView (macOS) / WebKitGTK (Linux) |
| 应用与业务逻辑 | Rust (edition 2021), serde |
| RDP 协议 | FreeRDP（libfreerdp2，后续以 FFI 封装在 `crates/easyrdp-rdp`） |
| 配置解析 | 自研 `.rdp` 文本解析器（UTF-16LE / UTF-8，encoding_rs） |

## 构建

前置要求：Rust toolchain（rustup）、Node.js + pnpm、各平台 WebView 依赖（Windows 需 WebView2 Runtime）。

```bash
# 开发模式（首次运行会拉取依赖）
cargo tauri dev

# 发布构建
cargo tauri build

# 仅运行单元测试
cargo test
```

## 目录说明

```
easyrdp-client/
├── Cargo.toml              # Cargo workspace 根
├── crates/
│   └── easyrdp-core/       # 核心库：SessionProfile、.rdp 导入/导出、校验
│       └── src/
│           ├── profile.rs      # 会话配置模型 + validate()
│           ├── rdp_import.rs   # mstsc .rdp 文件解析（含 UTF-16LE）
│           └── export.rs       # SessionProfile → .rdp 文本导出
├── src-tauri/              # Tauri 2 应用壳
│   ├── tauri.conf.json     # 最小 Tauri 配置
│   └── src/
│       ├── main.rs             # Tauri command：import_rdp / list_profiles
│       └── ui/
│           ├── index.html      # 最小前端页面
│           └── main.js         # 调用 IPC command 展示结果
├── docs/
│   └── ARCHITECTURE.md     # 四层架构说明 + FreeRDP 集成计划
└── README.md
```

## 许可证

MIT，见 [LICENSE](LICENSE)。
