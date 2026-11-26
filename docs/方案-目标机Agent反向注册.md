# 方案 — 目标机 Agent 反向注册（EasyRDP 云网关）

> 状态：**立项方案（待评审）**
> 日期：2026-11-22
> 关联：F4 网关接入（v0.3.7+）、R-20261120-009 gw-admin、F-R1-49 部署拓扑口径
> 适用仓库：`EasyRDP/easyrdp-gateway`（服务端）、新增 `EasyRDP/easyrdp-agent`（目标机端）

---

## 1. 背景与问题

当前云网关隧道链路为：

```
EasyRDP客户端 ⇄ 网关(WSS 443) --网关主动TCP连接--> [[resources]] 登记 host:port
```

`tunnel.rs::tunnel_handler` 第 2 步由网关**主动** `TcpStream::connect(backend)`。当目标 RDP 主机位于无公网 IP 的内网（只能出网、入站不可达）时，网关够不着它，云网关链路断裂。当前架构没有"目标机反向外连"通道。

**目标**：目标机运行 Agent，主动外连网关建立持久 WSS 隧道；`tunnel/{resource_id}` 命中 agent 型资源时，网关不再主动连后端，而是复用该反向隧道转发 RDP 字节流。

## 2. 总体架构

```
EasyRDP客户端 ⇄ 网关 WSS /tunnel/{rid}
                     │  (资源解析: mode="agent")
                     ▼
              AgentChannel 注册表（内存，按 resource_id 索引）
                     ▲
                     │ WSS /agent/connect（持久，目标机外连，Bearer AgentToken）
                     ▼
              easyrdp-agent（目标机，Windows 服务）
                     │ 本地 TCP
                     ▼
              127.0.0.1:3389 (TermService)
```

核心变化一句话：**网关→后端的主动连接，替换为"网关→Agent 的多路复用 DIAL 指令"，实际 TCP 连接由 Agent 在目标机本地发起。**

## 3. 线缆协议（Agent ↔ 网关）

复用现有 WSS + rustls 栈；**Binary 帧承载业务，Text 帧忽略**（与既有泵语义一致）。Binary 帧统一头部：

```
偏移  长度  字段
0     1    type
1     8    session_id (u64 BE, 每条客户端会话唯一，网关分配)
9     4    payload_len (u32 BE)
13    N    payload
```

| type | 方向 | 含义 |
|------|------|------|
| 0x01 DIAL | GW→Agent | payload = JSON `{backend:"127.0.0.1:3389"}`（来自资源配置），请求 Agent 建立一条本地 TCP |
| 0x02 OPEN_OK | Agent→GW | 本地 TCP 已建立，session 开始 |
| 0x03 OPEN_ERR | Agent→GW | payload = 错误摘要；网关回客户端 502 语义 |
| 0x04 DATA | 双向 | payload = 透传字节 |
| 0x05 CLOSE | 双向 | 关闭该 session |
| 0x06 PING / 0x07 PONG | 双向 | 心跳（30s 间隔，连续 3 次未收到 → 判死） |

约束：
- 单 Agent 连接并发 session 上限 8（配置可调）；每 session 流量与现网关一致（16KiB 缓冲切帧）。
- DIAL 的 backend **只来自网关配置**，Agent 不接受任意地址——防 Agent 被滥用为内网跳板。
- 心跳由网关侧计时器驱动（复用 axum WS 底层 Ping/Pong 之外的显式应用层帧，避免代理层剥 Ping）。

## 4. 服务端改造点（easyrdp-gateway）

| 模块 | 改动 |
|------|------|
| `resources.rs` | `ResourceConfig` 增加 `mode: "direct"（默认）|"agent"`、`agent_local_backend`（默认 `127.0.0.1:3389`）。`resolve_backend` 对 agent 型资源返回 `BackendResolution::Agent(resource_id)` |
| `resources.rs` 新增 | **动态资源注册表**：与静态 `[[resources]]` 并列的运行时资源层（内存 + 落盘持久化 `resources.dyn.toml`，网关重启自动恢复），含 `owner`(用户邮箱)、`mode`、后端/enrollment 状态；`resolve_backend`/`list` 合并静态+动态两层，动态资源天然绑定 owner 邮箱（grants 校验 owner 一致才放行 issue/tunnel） |
| 新增 `agent.rs` | `AgentRegistry`（`DashMap<resource_id, AgentConn>`）：注册/摘除/健康度；`GET /agent/connect` WS 升级路由，**独立 auth_guard**：校验 `role=agent` 的 AgentToken JWT；心跳计时；按帧协议分发 |
| `tunnel.rs` | 第 2 步分支：agent 型资源 → 从注册表取健康通道 → 分配 session_id → 发 DIAL → 等 OPEN_OK（10s 超时，超时审计 `agent_dial_timeout`）→ 用现有 `pump_ws_to_tcp/pump_tcp_to_ws` 语义桥接（TCP 半边替换为"Agent 帧读写适配器"，实现 `AsyncRead/AsyncWrite`，泵函数零改动） |
| `auth.rs` | AgentToken 签发：独立 claim 集 `{sub: agent_id, roles:["agent"], rid: resource_id, exp}`；与用户 JWT 不同 audience，互不可用 |
| `admin.rs` | 新增 `POST /admin/resource`（X-Admin-Key，供官网 PHP 回环调用）：动作 `register`（动态注册资源：mode=direct/agent）、`delete`、`status`（agent 在线状态查询）；注册时同步写 grants（owner 邮箱），**保持"验证通过即自动审批"现状，全程无人工** |
| `config.rs` | `[agent]` 段：`enabled`、`max_sessions_per_conn`、`heartbeat_secs`、`dial_timeout_secs`、`max_resources_per_user` |
| `audit.rs` | 新事件：`agent_online` / `agent_offline` / `agent_dial` / `agent_dial_error` / `agent_dial_timeout` |
| `policy.rs` | 不变——用户侧 connect 仍走资源×角色矩阵；agent 型资源同样受控 |

**故障语义**：Agent 掉线 → 注册表摘除 → 该资源 `tunnel/{rid}` 立即 502（错误文案"目标机 Agent 不在线"），客户端错误映射表（R-20261121-001）新增一条。Agent 重连后自动恢复，无需网关操作。

## 5. 目标机端（新增 crate easyrdp-agent）

- **形态**：Rust + tokio 控制台/服务双模式，Windows 注册为服务（`sc create`，开机自启），配置文件 `agent.toml`：

```toml
[gateway]
url = "https://easyrdp.cn"        # 自动解析 wss 443，复用客户端的完整地址粘贴解析语义
token = "<AgentToken>"            # 站长后台签发，绑定 resource_id
[tunnel]
resource_id = "rdp-office-01"
local_backend = "127.0.0.1:3389"
```

- **行为**：启动 → WSS `/agent/connect` → 在线待命；断线指数退避重连（1s→2s→…→60s 封顶）；收到 DIAL → `TcpStream::connect(local_backend)`（5s 超时）→ OPEN_OK → 双向 DATA 泵。
- **依赖**：复用 gateway 侧 axum-client/tokio-tungstenite/rustls 依赖版本，不引新栈。
- **平台**：首版 Windows x64（与客户端发布节奏一致），Linux 后续跟进。

## 6. 资源注册与用户流程（自助、零人工、公网/内网统一）

### 6.0 现状缺口与本期新增范围（资源管理全链路都是新的）

**现状事实**（以 2026-11-22 源码为准）：

- 资源列表是 `api/gateway/keys.php` 里的**静态数组**，gw-admin.html 只有「我的资源」只读展示 + 签发/吊销授权，**没有任何资源增删改**；
- `admin.html` 站长后台**没有用户资源管理功能**（只有用户/审计等卡片）；
- 资源未落库（不在 users.db），靠静态配置，无法支持自助登记。

**本期必须新增的资源管理全链路**：

| 层 | 新增内容 |
|----|----------|
| users.db | 新表 `resources`：`id(u-前缀)/owner_email/label/mode(direct\|agent)/backend/status(pending\|online\|offline\|revoked)/created_at`，自举建表（沿用 users/grants 惯例） |
| issue.php | 新 action：`register_resource`（direct：探测后落库+写 grants；agent：落库+返回 enrollment token）、`update_resource`（改 label/backend，mode 不可改——改接入方式应删了重登记，避免状态机混乱）、`delete_resource`（软删 revoked + 清 grants + agent 型通知网关摘除）、`my_resources` 升级（合并静态 keys.php 资源与 DB 资源，返回 mode/status/在线态） |
| gw-admin.html | 「我的资源」升级为资源管理卡片：列表（名称/ID/形态/在线状态徽标）+ **登记资源**表单（直连：名称+host:port；内网：名称→发 enrollment token）+ 编辑（label/backend）+ 删除（确认框）+ agent 资源的 enrollment token 结果面板（复用现有 issue-panel 风格）+ 7 天未上线「待清理」红标 |
| admin.html 站长后台 | 新增「用户资源管理」卡片：全量资源列表（owner/形态/状态/创建时间，按 owner 或 ID 搜索）、删除/封禁资源、强制摘除 Agent 在线态、资源审计跳转；调网关 `/admin/resource` 同款端点 |
| easyrdp-gateway | `POST /admin/resource`（register/update/delete/status，X-Admin-Key）+ 动态资源注册表（内存+落盘，见第 4 节） |

### 6.1 用户登记流程（两形态）

### 6.1 用户在 gw-admin「我的资源」新增"登记资源"

用户二选一：

**形态 A — 目标机在公网（有公网 IP/已做端口映射）→ 直连模式**

1. 填 `资源名称` + `目标地址 host:port`（如 `203.0.113.7:3389` 或 `myhome.ddns.net:33890`）；
2. PHP 回环调网关 `POST /admin/resource {mode:"direct", backend}`，网关**注册前探测**：`TcpStream::connect` 3s 试连成功才落库（失败直接报"目标不可达"，不让用户带着死资源走）；
3. 注册成功 → grants 自动写入（owner=当前邮箱）→ 资源出现在「我的资源」→ 走既有签发 token → 客户端连接流程，零变化。

**形态 B — 目标机在内网（无公网 IP，仅可出网）→ Agent 反向模式**

1. 填 `资源名称`，接入方式选"内网（Agent 反向）"；
2. PHP 回环调网关 `POST /admin/resource {mode:"agent"}`，网关生成 **enrollment token**（一次性、10 分钟、绑定该 resource_id）展示给用户；
3. 用户在目标机安装 `easyrdp-agent`，运行 `easyrdp-agent --enroll <token>` → Agent 外连网关上线 → 资源状态从「待接入」翻为「在线」，grants 同样自动写入；
4. 之后与形态 A 相同：gw-admin 签发 token → 客户端连。**用户侧体验两种形态完全一致，唯一区别是内网形态多一步在目标机装 Agent。**

### 6.2 一致性保证

- 资源 ID 统一格式：`u-<短随机>`（动态资源前缀 `u-`，与静态 `[[resources]]` 命名空间隔离，防冲突/防覆盖静态资源）；
- 「我的资源」列表 = 静态授权资源（现状保留）+ 用户自助登记资源，同一列表、同一签发按钮；
- 用户 token 签发、客户端连接、gw-admin UI **均无需任何改动感知 mode**——网关在隧道层自行选择"主动连后端"或"走 Agent 通道"。

### 6.3 防滥用（自助开放的边界）

- 每用户动态资源配额（默认 3 个，`max_resources_per_user` 可调）——**配额只限"登记资源"的台数，token 签发不限次**：登记 1 台目标机后可无限次签发短期/长期 token，配额仅约束接入的目标机数量，防网关被当批量穿透农场使用；
- direct 形态 SSRF 防线：注册探测拒绝私网/环回/链路本地地址（复用现有 `resources.rs` SSRF 白名单逻辑）；agent 形态后端固定 `127.0.0.1:3389`，天然无 SSRF 面；
- 资源与 owner 绑定：issue/tunnel 校验 grants owner，用户 A 无法签发用户 B 的资源 token；
- 探测失败的 direct 资源不落库；agent 资源 7 天无 Agent 上线自动标记「待清理」并邮件提醒（不做静默删除）。

## 7. 安全要点

- AgentToken 与用户 JWT 分 audience/分 role，策略矩阵对 agent 通道不生效（agent 是资源宿主，不是访问者）。
- Agent 只能 DIAL 配置内的 `local_backend`，不可作内网任意端口跳板。
- DIAL/OPEN_ERR/CLOSE 全量审计，含 agent_id、session_id、目标后端摘要。
- enrollment 一次性 + 短时效；AgentToken 泄露可在后台吊销（JWT 黑名单或短有效期+自动续签二选一，**推荐：默认 24h 有效期 + Agent 心跳时静默续签**，吊销后下个心跳失效）。

## 8. 分期与验收

| 期 | 内容 | 验收 |
|----|------|------|
| P1 | 线缆协议定稿 + `agent.rs`（注册表/路由/帧分发/心跳）+ 单测（mpsc 假 Agent 验证 DIAL/DATA/CLOSE 语义） | `cargo test -p easyrdp-gateway` 全过 |
| P2 | `tunnel.rs` agent 分支 + AsyncWrite 适配器 + 错误映射 | 回环 e2e：假 Agent 进程 ↔ 网关 ↔ 客户端桥，字节级一致 |
| P3 | easyrdp-agent crate + enroll + Windows 服务 + DPAPI | 真机：内网目标机（无公网 IP）+ 云网关全链路出帧 |
| P4 | ①users.db `resources` 表 + issue.php 资源增删改 action ②gw-admin 资源管理 UI（列表/登记/编辑/删除/enrollment 面板）③admin.html 用户资源管理卡片 ④网关 `/admin/resource` + 动态注册表 | 用户全程自助完成公网直连与内网 Agent 两种资源登记、签发、连通，站长零介入；站长后台可查/删/封用户资源；审计可查 |

工作量估计：P1–P2 约 2 个开发日，P3 约 2 日，P4 约 3 日（资源管理全链路：DB 表、PHP action、gw-admin UI、admin 后台卡片、网关动态注册表）；回归沿用 F4 全量清单 + 新增资源管理 CRUD 冒烟清单。

## 9. 风险与备选

- **WS 中间层空闲断连**：显式应用层 PING（非协议 Ping）规避；NAT 超时可配心跳间隔。
- **单 Agent 连接为单点**：V2 支持 Agent 双连接（同 resource 双活，网关任选健康者）；首版单连接+快速重连已满足目标场景。
- **备选方案（已否决）**：让用户在网关上配 frp——引入第三方组件、绕过审计与策略矩阵，违背网关"统一鉴权/审计"定位。

## 10. EasySSH 关系

按 F-R1-49 口径：EasySSH 网关是独立源码、独立实例、仅接口协议兼容。本方案的**线缆协议（第 3 节）作为两项目共享的 wire spec**，EasySSH 侧按同一协议独立实现（见其仓库内同名方案文档），代码互不共用。
