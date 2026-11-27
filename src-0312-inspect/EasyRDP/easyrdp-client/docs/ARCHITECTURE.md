# EasyRDP 架构说明

## 四层架构

```
┌─────────────────────────────────────────────┐
│ 第 1 层: UI 壳 (Tauri 2)                     │
│  WebView2 / WKWebView / WebKitGTK           │
│  HTML + JS，通过 invoke() 调用 Rust command  │
└──────────────────┬──────────────────────────┘
                   │ Tauri IPC
┌──────────────────▼──────────────────────────┐
│ 第 2 层: 会话引擎 (src-tauri)                │
│  command: import_rdp / list_profiles        │
│  后续: connect / disconnect / 事件回传帧数据  │
└──────┬───────────────────────────┬──────────┘
       │                           │
┌──────▼──────────────┐  ┌─────────▼─────────┐
│ 第 3 层: Profile 配置 │  │ 第 4 层: 网关集成   │
│ crates/easyrdp-core │  │ (规划)             │
│ SessionProfile      │  │ RD Gateway 隧道    │
│ .rdp 导入/导出/校验  │  │ OTP Cookie / SSO   │
└─────────────────────┘  └───────────────────┘
       │
┌──────▼──────────────────────────────────────┐
│ 协议层 (规划: crates/easyrdp-rdp)            │
│ FreeRDP (libfreerdp2) FFI 封装               │
└─────────────────────────────────────────────┘
```

### 第 1 层 — UI 壳

Tauri 2 应用，前端使用原生 HTML/JS（MVP 阶段无框架）。深色主题
（背景 `#0f1117`，主色 `#3b82f6`）。Windows 下依赖 WebView2 Runtime，
macOS 用系统 WKWebView，Linux 用 WebKitGTK。

### 第 2 层 — 会话引擎

`src-tauri/src/main.rs` 提供 `#[tauri::command]`。MVP 已实现：

- `import_rdp(path)`：读取 .rdp 文件 → 解析 → 校验 → 存入内存 ProfileStore
- `list_profiles()`：返回全部已导入配置

后续将增加 `connect(profile_id)`、断开管理、以及把 FreeRDP 帧缓冲
通过 Tauri 事件 / 自绘通道送到前端画布。

### 第 3 层 — Profile 配置

`crates/easyrdp-core`：

- `SessionProfile`：name / host / port(默认 3389) / username / auth_method
  (Password|Sso|SmartCard) / resolution (FitWindow|Fullscreen|Custom) /
  color_depth / redirect (clipboard|printers|drives|camera) /
  gateway (Option\<GatewayConfig{hostname, auth: OtpCookie|Inherit}\>)
- `.rdp` 文本解析（UTF-16LE / UTF-8 自动识别，`key:type:value` 行格式，
  未知键忽略）与反向导出，导入↔导出往返一致。
- `validate()`：主机非空、端口 1-65535（u16 类型本身保证上界）。

### 第 4 层 — 网关集成（规划）

RD Gateway 场景：先与 `gateway.hostname` 建立 TLS 隧道，认证方式：

- `OtpCookie`：网关签发的一次性密码以 HTTP Cookie 形式携带
- `Inherit`：复用主会话凭据

随后在隧道内进行标准 RDP 握手。

## FreeRDP 集成计划（MVP 之后）

MVP 阶段**不含**协议实现，仅完成配置与壳工程。后续在
`crates/easyrdp-rdp` 中以 FFI 封装 FreeRDP（libfreerdp2）：

1. `freerdp-sys`（或绑定 crate）暴露 `freerdp_new`、
   `freerdp_connect`、`freerdp_check_event_handles` 等核心 API。
2. 安全 Rust 封装：`RdpConnection`（RAII 生命周期）、
   `RdpContext`（预连接回调里注入凭据/网关参数）。
3. 帧通道：FreeRDP 的 `rfx/neven` 解码回调产出 RGBA 帧缓冲，
   经跨线程通道送到 Tauri 层，前端 canvas 渲染。
4. 输入回传：键盘/鼠标事件从前端 → Tauri command → FFI
   `freerdp_input_send_*`。
5. 剪贴板/驱动器重定向按 `SessionProfile.redirect` 开关联动
   FreeRDP 的对应通道配置。

## 目录对照

| 路径 | 层 | 职责 |
|---|---|---|
| `src-tauri/src/ui/` | UI 壳 | 页面与 IPC 调用 |
| `src-tauri/src/main.rs` | 会话引擎 | Tauri command、ProfileStore |
| `crates/easyrdp-core` | Profile 配置 | 模型、导入/导出、校验 |
| `crates/easyrdp-rdp`（规划） | 协议 | FreeRDP FFI |
