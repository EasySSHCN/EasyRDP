# easyrdp-agent（EasyRDP 目标机 Agent，R-20261122-006 P3）

Windows 服务形态的 EasyRDP 反向隧道 Agent：目标机主动外连云网关
`WSS /agent/connect`，把网关下发的 DIAL 复用为本地 TCP 连接（默认
`127.0.0.1:3389` TermService），实现无公网 IP 内网主机的云网关接入。
方案文档：`EasyRDP/docs/方案-目标机Agent反向注册.md`（§3 wire、§5 目标机端）。

## 快速上手

```powershell
# 1) 准备 agent.toml（从 config.example.toml 拷贝，填 gateway.url 与 tunnel.resource_id）
# 2) 用一次性 enrollment token 换取 AgentToken（DPAPI 加密写回 agent.toml）
easyrdp-agent --enroll <ENROLL_TOKEN> --config C:\path\agent.toml
# 3) 控制台调试
easyrdp-agent --config C:\path\agent.toml
# 4) 注册为 Windows 服务（开机自启）
sc create EasyRDPAgent binPath= "<exe绝对路径> --service --config C:\path\agent.toml" start= auto
sc start EasyRDPAgent
```

## 模块

| 文件 | 职责 |
|------|------|
| `src/wire.rs` | agent-wire/1 帧编解码 + `FrameIo` 抽象 |
| `src/session.rs` | 会话状态机：PING/PONG 心跳、DIAL→本地 TCP、DATA 双向泵、静默判死 |
| `src/ws.rs` | tokio-tungstenite（rustls/webpki-roots）FrameIo 适配 + 建连 |
| `src/backoff.rs` | 指数退避重连（1s→…→60s 封顶） |
| `src/enroll.rs` | `--enroll`：enrollment token → AgentToken |
| `src/dpapi.rs` | AgentToken DPAPI 落盘（`dpapi:<base64>` 内联） |
| `src/config.rs` | agent.toml 解析 + token 解析 + `connect_ws_url` |
| `src/service.rs` | Windows 服务（windows-service SCM，STOP 优雅退出） |

## 测试

```
cargo test -p easyrdp-agent
```

覆盖：帧编解码（粘包/半帧/非法 type/超限 payload）、退避序列、配置解析与
token 三形态、enroll 请求/拒绝/错误路径、DPAPI 加解密回环（Windows）、
DIAL 语义（允许/拒绝/拒连/坏 payload/EOF CLOSE/64KiB 双向转发）、心跳与静默
判死、断线重连——会话逻辑全部经 `FrameIo` mpsc 内存回环驱动，测试语义 =
运行语义。
