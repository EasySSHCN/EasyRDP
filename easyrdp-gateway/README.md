# EasyRDP 云网关（easyrdp-gateway）

EasyRDP 云网关：基于 **Rust + axum + tokio** 的 443 TLS 隧道 + 预认证 + 真实 TCP 转发 + 审计服务，
对齐微软 RD Gateway（Remote Desktop Gateway）的"HTTPS 隧道承载 RDP"模型。

> 隧道已完成 **WebSocket ⇄ 内网 RDP 主机 TCP** 双向转发；OIDC 登录仍为占位
> （`"<sub>:<role1>,<role2>"` 解析，便于联调），后续接入真实 IdP。

## 一、架构说明

```
 客户端（RDP Client / EasyRDP 连接器）
        │  WSS（443, TLS 由 Nginx 终止）
        ▼
 ┌───────────────────────────┐
 │  Nginx 反代 (443)          │  TLS 终止 + WebSocket Upgrade 转发
 └────────────┬──────────────┘
              │ HTTP/WS (8443)
              ▼
 ┌───────────────────────────────────────┐
 │  easyrdp-gateway (axum)                │
 │  ├─ /api/auth/login   认证             │  OIDC code 占位 -> 签发短期 JWT
 │  ├─ auth_guard        鉴权             │  Bearer 校验中间件
 │  ├─ /api/resources    资源列表         │  需认证；只返回 id/name（不含后端地址）
 │  ├─ /tunnel/{id}      隧道             │  解析后端 -> 连接 RDP 主机 -> WS 升级 ->
 │  │                                     │    WS Binary 帧 ⇄ TCP 字节流 双向泵
 │  ├─ /api/audit        审计             │  内存环形缓冲查询（admin 限定），
 │  │                                     │    连接失败也记录（含错误摘要）
 │  └─ resources/policy  资源目录+策略     │  SSRF 白名单：仅私网 IP/localhost；
 │                                        │  重定向矩阵：运维全开 / 外包全禁
 └────────────┬──────────────────────────┘
              │ TCP（内网隔离区）
              ▼
        内网 RDP 主机（10.x / 172.16-31.x / 192.168.x / localhost）
```

核心链路：客户端 ⇄ 网关(WSS 443) ⇄ 内网 RDP 主机。
客户端不直连内网主机；所有 RDP 流量经网关鉴权、审计、策略过滤后转发。

## 二、后端地址解析与 SSRF 防护

`/tunnel/{resource_id}` 按以下顺序解析后端（见 `src/resources.rs`）：

1. **资源注册表**（`config.toml` 的 `[[resources]]`）：`id -> backend("host:port")`，
   管理员配置的可信来源；
2. **显式直连**：resource_id 本身是 `"host:port"` 格式，且主机通过
   `is_allowed_backend` 白名单校验，仅允许：
   - `10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`
   - `localhost`、`127.0.0.0/8`、`::1`

   DNS 域名、公网 IP、链路本地地址等一律拒绝，防止 SSRF 任意探测。

## 三、认证流程（对齐 GatewayEncryptedAuthCookie 概念）

RD Gateway 使用短期加密 cookie（GatewayEncryptedAuthCookie）标识一次已认证会话；
本网关用**短期 JWT**承载同等语义：

1. **OIDC 登录**：客户端先在 IdP 完成 OIDC 授权码流程（含 **MFA**），获得一次性 `code`。
2. **换取网关 token**：客户端 `POST /api/auth/login` 提交 `{ oidc_code, mfa_code? }`。
   真实实现中网关向 `oidc_issuer` 换取并校验 id_token（签名/aud/MFA 结论），
   当前用 `"<sub>:<role1>,<role2>"` 占位解析身份（便于联调）。
3. **签发短期 cookie**：网关签发含 `sub` 与 `roles` claims 的 JWT，
   默认 **10 分钟过期**（`token_ttl_seconds = 600`）。
4. **隧道使用**：客户端 `GET /tunnel/{resource_id}`（WebSocket 升级）携带
   `Authorization: Bearer <jwt>`，校验失败即 401，隧道不建立。
5. **过期重签**：token 过期后客户端须重新走 OIDC + MFA。

## 四、curl 测试示例

```bash
# 1. 登录换取短期 token（占位 code 格式：sub:roles）
TOKEN=$(curl -s -X POST http://127.0.0.1:8443/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"oidc_code":"zhangsan:ops,admin"}' | jq -r .token)

# 2. 资源列表（需认证；只含 id/name，不含后端地址）
curl -s http://127.0.0.1:8443/api/resources -H "Authorization: Bearer $TOKEN"
# => {"resources":[{"id":"rdp-win01","name":"rdp-win01"},...]}

# 3. 建立隧道（WebSocket，须带 Bearer；wscat 示例，二进制帧透传到后端 RDP 主机）
npx wscat -c "ws://127.0.0.1:8443/tunnel/rdp-win01" \
  -H "Authorization: Bearer $TOKEN"

# 4. 显式私网地址直连（白名单内才允许）
npx wscat -c "ws://127.0.0.1:8443/tunnel/192.168.1.20:3389" \
  -H "Authorization: Bearer $TOKEN"

# 5. 公网地址会被拒绝（SSRF 防护，404）
curl -i "http://127.0.0.1:8443/tunnel/8.8.8.8:3389" -H "Authorization: Bearer $TOKEN"

# 6. 审计查询（仅 admin 角色；连接失败事件含 error 摘要）
curl -s http://127.0.0.1:8443/api/audit -H "Authorization: Bearer $TOKEN"
```

## 五、模块说明

| 模块 | 职责 |
|------|------|
| `src/main.rs` | 加载 config.toml（环境变量/默认值回退）、路由挂载、共享状态 |
| `src/config.rs` | `GatewayConfig` TOML 解析（server/auth/audit/`[[resources]]`）；3 个单测 |
| `src/auth.rs` | `POST /api/auth/login`、短期 JWT 签发/校验、`auth_guard` 中间件；3 个单测 |
| `src/resources.rs` | 资源注册表、`resolve_backend`、SSRF 白名单 `is_allowed_backend`、`parse_host_port`；6 个单测 |
| `src/tunnel.rs` | `GET /tunnel/{id}` WS 升级 + TCP 转发（`pump_ws_to_tcp` / `pump_tcp_to_ws` 双向泵，Text 帧忽略）、`GET /api/resources`；5 个单测；后端连接失败审计 `tunnel_backend_error` |
| `src/audit.rs` | 内存环形缓冲审计（含可选 `error` 字段），`GET /api/audit`（admin 限定）；2 个单测 |
| `src/policy.rs` | `check_redirect(role, redirect_kind) -> Allow/Deny`；3 个单测 |
| `config.example.toml` | 监听地址、JWT secret、`[[resources]]` 资源注册表示例 |

共 22 个单元测试（`cargo test` 全绿）。

## 六、配置示例（config.toml）

```toml
[server]
listen_addr = "0.0.0.0:8443"

[auth]
jwt_secret = "CHANGE_ME_PLACEHOLDER_DO_NOT_USE_IN_PRODUCTION"  # openssl rand -base64 64
token_ttl_seconds = 600

[audit]
ring_capacity = 4096

[[resources]]
id = "rdp-win01"
name = "Windows 跳板机"
backend = "10.0.0.5:3389"

[[resources]]
id = "rdp-linux01"
name = "Linux 桌面"
backend = "192.168.1.20:3390"
```

配置文件路径：`EASYRDP_CONFIG`（默认 `./config.toml`）；文件缺失时回退
`EASYRDP_JWT_SECRET` / `EASYRDP_LISTEN` 环境变量与默认值。

## 七、部署方式（Docker / Nginx 反代）

1. 复制配置并修改敏感项：

   ```bash
   cp config.example.toml config.toml   # 修改 jwt_secret（openssl rand -base64 64）
   ```

2. Nginx 监听 443 TLS，`proxy_pass http://gateway:8443`，开启
   `Upgrade`/`Connection` 头支持 WebSocket，`proxy_read_timeout 3600s` 放宽长连接。

3. 启动：

   ```bash
   docker compose up -d --build
   ```

## 八、本地运行

```bash
cd easyrdp-gateway
cargo test                        # 22 个单元测试
cargo build --release
./target/release/easyrdp-gateway  # 默认 0.0.0.0:8443
```

> Windows 沙箱注意：若子进程写 `%TEMP%` 被拒（MSVC 编译 ring 报 D8050），
> 先把 TEMP/TMP 指到工作区，如 `target\tmp`，再执行 cargo 命令。

## 九、TODO（进入下一阶段）

- [ ] 真实 OIDC code 换 token + id_token 校验 + MFA 结论校验
- [ ] 审计日志落地到 append-only 存储 / SIEM 推送
- [ ] 自定义角色的重定向矩阵配置化（policy.rs 预留）；隧道会话接入角色级资源授权
- [ ] 网关自身 TLS 终止支持（证书热加载）
- [ ] RDP PDU 层面的空闲/流量限制与会话上限
