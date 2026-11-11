# EasyRDP 云网关（easyrdp-gateway）

EasyRDP 云网关骨架：基于 **Rust + axum + tokio** 的 443 TLS 隧道 + 预认证 + 审计服务，
对齐微软 RD Gateway（Remote Desktop Gateway）的"HTTPS 隧道承载 RDP"模型。

> ⚠️ 骨架阶段：结构与核心代码已就绪，隧道双向循环目前为 **echo 占位**，
> 尚未对接内网 RDP 主机 TCP 转发，也未接入真实 OIDC IdP。不要求本阶段联网编译。

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
 ┌───────────────────────────┐
 │  easyrdp-gateway (axum)    │
 │  ├─ /api/auth/login  认证  │  OIDC code 占位 -> 签发短期 JWT
 │  ├─ auth_guard       鉴权  │  Bearer 校验中间件
 │  ├─ /tunnel/{id}     隧道  │  WS 升级 -> 审计 -> (占位)echo 循环
 │  ├─ /api/audit       审计  │  内存环形缓冲查询（admin 限定）
 │  └─ policy          策略   │  重定向矩阵：运维全开 / 外包全禁
 └────────────┬──────────────┘
              │  TODO: TCP 转发（内网隔离区）
              ▼
        内网 RDP 主机（rdp-win01 / rdp-linux01 ...）
```

核心链路：客户端 ⇄ 网关(WSS 443) ⇄ 内网 RDP 主机。
客户端不直连内网主机；所有 RDP 流量经网关鉴权、审计、策略过滤后转发。

## 二、认证流程（对齐 GatewayEncryptedAuthCookie 概念）

RD Gateway 使用短期加密 cookie（GatewayEncryptedAuthCookie）标识一次已认证会话；
本网关用**短期 JWT**承载同等语义：

1. **OIDC 登录**：客户端先在 IdP 完成 OIDC 授权码流程（含 **MFA**），获得一次性 `code`。
2. **换取网关 token**：客户端 `POST /api/auth/login` 提交 `{ oidc_code, mfa_code? }`。
   真实实现中网关向 `oidc_issuer` 换取并校验 id_token（签名/aud/MFA 结论），
   骨架阶段用 `"<sub>:<role1>,<role2>"` 占位解析身份（便于联调）。
3. **签发短期 cookie**：网关签发含 `sub`（主体）与 `roles`（角色）claims 的 JWT，
   默认 **10 分钟过期**（`token_ttl_seconds = 600`）。该 JWT 即"网关加密认证 cookie"。
4. **隧道使用**：客户端 `GET /tunnel/{resource_id}`（WebSocket 升级）时携带
   `Authorization: Bearer <jwt>`，`auth_guard` 中间件校验签名与有效期，
   失败即 401，隧道不建立。
5. **过期重签**：token 过期后客户端须重新走 OIDC + MFA（符合最小暴露窗口原则）。

示例：

```bash
# 登录换取短期 token（占位 code 格式：sub:roles）
curl -s -X POST http://127.0.0.1:8443/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"oidc_code":"zhangsan:ops,admin","mfa_code":"123456"}'
# => {"token":"eyJ...","expires_in":600,"token_type":"Bearer"}

# 建立隧道（WebSocket，须带 Bearer）
curl -s http://127.0.0.1:8443/tunnel/rdp-win01 \
  -H 'Authorization: Bearer eyJ...' \
  -H 'Upgrade: websocket' ...

# 审计查询（仅 admin 角色）
curl -s http://127.0.0.1:8443/api/audit -H 'Authorization: Bearer eyJ...'
```

## 三、模块说明

| 模块 | 职责 |
|------|------|
| `src/main.rs` | axum 启动（默认 `0.0.0.0:8443`）、路由挂载、共享状态 |
| `src/auth.rs` | `POST /api/auth/login`、短期 JWT 签发/校验、`auth_guard` 中间件；3 个单测（签发含 claims / 校验通过 / 过期拒绝） |
| `src/tunnel.rs` | `GET /tunnel/{resource_id}` WS 升级，校验后审计 + 占位 echo 双向循环（TODO: TCP 转发） |
| `src/audit.rs` | 内存环形缓冲审计（时间/用户/事件/资源/来源IP），`GET /api/audit`（admin 限定）；2 个单测 |
| `src/policy.rs` | `check_redirect(role, redirect_kind) -> Allow/Deny`，默认矩阵：运维全开、外包全禁；3 个单测 |
| `config.example.toml` | 监听地址、JWT secret 占位、OIDC issuer/client_id 占位、TLS 证书路径占位 |
| `Dockerfile` / `docker-compose.yml` | 多阶段构建 + Nginx 反代 443→8443（见 compose 内注释） |

## 四、部署方式（Docker / Nginx 反代）

1. 复制配置并修改敏感项：

   ```bash
   cp config.example.toml config.toml   # 修改 jwt_secret（openssl rand -base64 64）
   mkdir certs && cp server.crt server.key certs/
   ```

2. 编写 `nginx/nginx.conf`（核心片段见 `docker-compose.yml` 尾部注释）：
   监听 443 TLS，`proxy_pass http://gateway:8443`，开启
   `Upgrade`/`Connection` 头支持 WebSocket，`proxy_read_timeout 3600s` 放宽长连接。

3. 启动：

   ```bash
   docker compose up -d --build
   ```

   Nginx 承接公网 443 TLS；网关仅在内网暴露 8443。
   也可让网关自己终止 TLS（预留 `tls.cert_path/key_path`），二选一。

## 五、本地运行（需先拉取依赖）

```bash
cd easyrdp-gateway
cargo run                     # 默认 0.0.0.0:8443；可用 EASYRDP_LISTEN 覆盖
cargo test                    # 运行 auth/audit/policy 单元测试
```

> 当前环境未执行过 `cargo build`（按任务要求仅静态产出）；
> 首次运行需联网拉取 crates.io 依赖。

## 六、TODO（进入下一阶段）

- [ ] 隧道 echo 占位替换为内网 RDP 主机 TCP 转发（tokio TcpStream 双向透传）
- [ ] 真实 OIDC code 换 token + id_token 校验 + MFA 结论校验
- [ ] 审计日志落地到 append-only 存储 / SIEM 推送
- [ ] 自定义角色的重定向矩阵配置化（policy.rs 预留）
- [ ] 网关自身 TLS 终止支持（证书热加载）
