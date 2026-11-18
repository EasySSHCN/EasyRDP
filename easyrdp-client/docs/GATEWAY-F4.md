# F4 网关接入（v0.3.7）——客户端经云网关连接 RDP

状态：**开发 + 自测完成**（2026-11-18）。不发布、不部署、不含真机联调。

## 方案选型与依据

| 候选 | 结论 | 依据 |
|---|---|---|
| A. FreeRDP 原生 RD Gateway | **否决** | 2.11.2 `<freerdp/settings.h>` 750-766 行键齐全（`FreeRDP_GatewayEnabled/Hostname/Port/Username/Password/AccessToken/HttpTransport/RpcTransport/...`），但其传输层走 **MS-TSGU** 协议（HTTP `/remoteDesktop/...` + RPC/NTLM）。本项目云网关（`easyrdp-gateway/src/tunnel.rs`）是自研协议：`GET /tunnel/{resource_id}` WS 升级 + `Authorization: Bearer <JWT>` + Binary 帧透传。报文/认证/端点完全不同，FreeRDP 无法直连。 |
| B. 本地回环桥（**采用**） | ✅ | 客户端进程内起 `127.0.0.1:<临时端口>` TCP 监听；每条 TCP 连接对一条网关 WS 隧道，双向字节泵（语义对齐 gateway 侧 `pump_ws_to_tcp`/`pump_tcp_to_ws`：Binary 透传、Text 忽略、Close→FIN）。FreeRDP 目标覆写 `ServerHostname=127.0.0.1` / `ServerPort=bridge_port`，原生 Gateway 设置一律不启用。零改动 FreeRDP 数据面。 |

决策依据同步写在代码注释：`crates/easyrdp-rdp/src/gateway_bridge.rs` 模块头。

## 链路

```
FreeRDP ──TCP──> 127.0.0.1:bridge_port（客户端回环桥，easyrdp-rdp::gateway_bridge）
     ──WS(wss/ws)──> wss://gw:443/tunnel/{resource_id}  (Authorization: Bearer JWT)
     ──axum pump──> 内网 RDP host:port
```

认证：
- `Token`：profile 内 JWT 直接作 Bearer；
- `UserPassword`：先 `POST /api/auth/login`（网关占位登录 `oidc_code=username`）换短期 JWT；
- `resource_id` 留空 → 以连接的 `host:port` 显式直连（网关 SSRF 私网白名单兜底）。

## 改动文件清单

客户端（easyrdp-client）：
- `crates/easyrdp-core/src/profile.rs`：`GatewayConfig` 扩展（port/use_tls/resource_id/token/username/password，serde default 向后兼容旧 JSON）；`GatewayAuth` 增 `Token`/`UserPassword`；校验扩充；`ws_url()`/`http_base()`；测试 5 个新增。
- `crates/easyrdp-core/src/export.rs` / `rdp_import.rs`：新字段适配（.rdp 扩展键 gatewayport/gatewayusessl/gatewayresourceid；导入回退 Inherit）。
- `crates/easyrdp-rdp/src/gateway_bridge.rs`（新增）：回环桥（`spawn_bridge` 同步 / `start_bridge` async）+ 泵 + 设置覆写计划 `plan_gateway_overrides` + 单测。
- `crates/easyrdp-rdp/src/lib.rs`：注册模块。
- `crates/easyrdp-rdp/src/ffi.rs`：connect 阶段启动桥并覆写连接目标；会话持有/关停 BridgeHandle；`plan_settings` 并入网关覆写；新增 2 个网关测试。附带修复示例 `examples/e2e_connect.rs` 缺 `NativeFrame` 分支的既有编译错误。
- `crates/easyrdp-rdp/Cargo.toml`：新增 tokio / tokio-tungstenite 0.21（与 axum 0.7 同 tungstenite 版本线）/ futures-util / serde_json。
- `src-tauri/src/main.rs`：`SaveArgs.gateway`（camelCase）+ `GatewayArgs::into_config`（凭据留空继承旧值）；`ConnectionView.gateway` 回显视图（不含 token/密码明文，只报 has_token/has_password）。
- `src/index.html`：连接表单新增「网关（可选）」折叠区（地址/端口/TLS/认证方式/Token/用户名/密码/资源 ID）。
- `src/ui/main.js`：保存提交 gateway；编辑回显；重置清空。

网关（easyrdp-gateway）：
- `src/tests_e2e.rs`（新增）：3 个端到端集成测试（见下）。
- `src/main.rs`：挂 `#[cfg(test)] mod tests_e2e`。
- `Cargo.toml`：dev-dependency `easyrdp-rdp`（客户端桥接模块，真链路复用）。

未改动：easyrdp-site、发布/打包脚本、git。

## 测试结果（Windows 本机实测）

| 命令 | 结果 |
|---|---|
| `cargo check -p easyrdp-app --features custom-protocol,native` | ✅ Finished（PKG_CONFIG_PATH/pkgconf 环境按 docs/WINDOWS-FREERDP-SETUP.md） |
| `cargo test -p easyrdp-core -p easyrdp-rdp -p easyrdp-app`（默认特性） | ✅ 43 + 30 + 21 全过（含桥泵语义 2 测、覆写计划 1 测、profile 网关校验/序列化兼容 5 测） |
| `cargo test -p easyrdp-rdp --features native` | 36 过 / 1 失败——失败项为**既有环境依赖测试** `openssl_legacy_provider_initializes_on_windows`（本机 vcpkg x64-windows 动态构建无 legacy provider 模块，与 F4 无关；网关相关 4 个 ffi 测试单独运行全过） |
| `cargo test`（easyrdp-gateway） | ✅ 33 全过，含 3 个端到端：①48KiB 任意字节经桥→WS 隧道→echo 回读逐字节一致（跨 16KiB 帧切分）；②后端主动推送原样到达客户端；③无效 JWT → 401 → 桥连接被断开 |

## 遗留项 / 真机联调清单

1. **真机联调**：`easyrdp-gateway` 部署侧配置 `[[resources]]` → 内网 Windows RDP 主机；客户端建带网关 profile；验证 FreeRDP 经桥完成完整 RDP 握手（X.224 连接请求/响应穿越 WS）与画面出帧。自测已覆盖字节层全链路，RDP 协议层需真机。
2. **wss/TLS**：桥目前仅明文 ws（tokio-tungstenite 未开 native-tls/rustls）；生产 443 TLS 需补 TLS client（或由本机 Nginx/stunnel 侧终止）。
3. **真实 OIDC**：`UserPassword` 走网关占位登录（`oidc_code=username`，角色恒 `user`）；对接真实 IdP 后需改为 code 换取 + MFA。
4. **网关凭据落盘**：token/密码目前随 profile 明文存 connections.json（主连接密码已是 DPAPI）；建议后续对网关凭据复用 `secret::protect`。
5. **多连接并发桥**：每会话一个桥实例（BridgeHandle 随会话生命周期），多会话并发未压测。
6. **既有失败测试**：`openssl_legacy_provider_initializes_on_windows` 在 x64-windows 动态构建下失败（注释自述其针对 x64-windows-static-md），建议改为按安装布局条件跳过。
