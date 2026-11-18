//! 端到端集成测试（F4 网关接入，v0.3.7）：
//! 本地起 gateway（axum + 真实 TCP echo 后端）→ 客户端桥接模块
//! （easyrdp-rdp::gateway_bridge，与 GUI 同一条代码路径）连它 →
//! 经桥写入 TCP 的字节应原样回读（WS Binary 帧透传 + 双向泵字节流一致）。
//!
//! 链路：TCP client → 127.0.0.1:bridge_port（客户端回环桥）
//!   → ws://127.0.0.1:gw_port/tunnel/echo（Bearer JWT，auth_guard 校验）
//!   → gateway pump_ws_to_tcp → echo 后端 TCP → pump_tcp_to_ws → 原路返回。

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::auth::AuthState;
    use crate::resources::ResourceRegistry;
    use crate::{tunnel, AppState};

    /// 启动 echo 后端（读到什么回什么），返回监听地址。
    async fn spawn_echo_backend() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        loop {
                            match sock.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    if sock.write_all(&buf[..n]).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    });
                }
            }
        });
        addr
    }

    /// 启动 axum gateway（auth + tunnel 路由）在 127.0.0.1 临时端口，
    /// 返回 (监听地址, AppState)——state 供测试直接签发 JWT。
    async fn spawn_gateway(registry: ResourceRegistry) -> (std::net::SocketAddr, Arc<AppState>) {
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: registry,
            admin_key: None,
        });
        let app = axum::Router::new()
            .merge(crate::auth::router(Arc::clone(&state)))
            .merge(tunnel::router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("gateway serve");
        });
        (addr, state)
    }

    /// 端到端：桥接模块经 gateway 隧道转发 TCP echo，字节流必须一致。
    #[tokio::test]
    async fn tunnel_echo_end_to_end_via_client_bridge() {
        // 1) echo 后端（模拟内网 RDP 主机的字节层）
        let backend = spawn_echo_backend().await;

        // 2) gateway：注册资源 "echo" -> backend，启动 axum 服务
        let registry = ResourceRegistry::default();
        registry.register("echo", backend.to_string());
        let (gw_addr, state) = spawn_gateway(registry).await;

        // 3) 签发短期 JWT（网关占位登录的真实产物）
        let token = state.auth.issue_token("alice", &["user"]).expect("issue token");

        // 4) 客户端桥接模块（与 GUI connect 同一入口，async 版 start_bridge）
        let gw = easyrdp_core::profile::GatewayConfig {
            hostname: gw_addr.ip().to_string(),
            port: gw_addr.port(),
            use_tls: false, // 测试用明文 ws
            auth: easyrdp_core::profile::GatewayAuth::Token,
            resource_id: "echo".into(),
            token: Some(token),
            ..Default::default()
        };
        let bridge = easyrdp_rdp::gateway_bridge::start_bridge(&gw, "127.0.0.1", backend.port())
            .await
            .expect("bridge spawn");

        // 5) TCP 客户端连回环桥 → 写入 → 回读校验字节流一致
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", bridge.local_port()))
            .await
            .expect("connect bridge");
        // 覆盖多帧场景：>16KiB（桥泵帧切分阈值）+ 二进制任意字节（含 0x00/0xFF）
        let pattern: Vec<u8> = (0..48 * 1024u32).map(|i| (i % 251) as u8).collect();
        sock.write_all(&pattern).await.expect("write pattern");

        let mut got = vec![0u8; pattern.len()];
        sock.read_exact(&mut got).await.expect("read echo");
        assert_eq!(got, pattern, "经网关 WS 隧道回读的字节流必须与写入一致");

        // 6) 半关闭 → echo EOF → 泵收尾链不挂死
        sock.shutdown().await.expect("shutdown");

        drop(bridge); // 关停回环桥
    }

    /// 反向流：后端先推数据（服务端主动包）经隧道到达客户端。
    #[tokio::test]
    async fn tunnel_backend_push_reaches_client() {
        // 后端：accept 后主动推一条"服务端横幅"
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = listener.local_addr().unwrap();
        let banner = b"RDP-SERVER-BANNER-0x03\x00\xff".to_vec();
        let expected = banner.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            sock.write_all(&banner).await.unwrap();
            // 保持连接直到客户端关闭（读到 EOF）
            let mut sink = [0u8; 64];
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(5), sock.read(&mut sink)).await;
        });

        let registry = ResourceRegistry::default();
        registry.register("push", backend_addr.to_string());
        let (gw_addr, state) = spawn_gateway(registry).await;
        let token = state.auth.issue_token("bob", &["user"]).unwrap();

        let gw = easyrdp_core::profile::GatewayConfig {
            hostname: gw_addr.ip().to_string(),
            port: gw_addr.port(),
            use_tls: false,
            auth: easyrdp_core::profile::GatewayAuth::Token,
            resource_id: "push".into(),
            token: Some(token),
            ..Default::default()
        };
        let bridge =
            easyrdp_rdp::gateway_bridge::start_bridge(&gw, "127.0.0.1", backend_addr.port())
                .await
                .unwrap();

        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", bridge.local_port()))
            .await
            .unwrap();
        // 客户端读到横幅（read_exact 到 banner 长度）
        let mut buf = vec![0u8; expected.len()];
        tokio::time::timeout(std::time::Duration::from_secs(5), sock.read_exact(&mut buf))
            .await
            .expect("receive banner within 5s")
            .expect("read banner");
        assert_eq!(buf, expected, "后端主动推送必须原样到达客户端");
        drop(sock);
        drop(bridge);
    }

    /// 无效 token：网关 auth_guard 401 → WS 升级失败 → 桥连接报错（TCP 直接断开）。
    #[tokio::test]
    async fn tunnel_rejects_bad_token() {
        let backend = spawn_echo_backend().await;
        let registry = ResourceRegistry::default();
        registry.register("secure", backend.to_string());
        let (gw_addr, _state) = spawn_gateway(registry).await;

        let gw = easyrdp_core::profile::GatewayConfig {
            hostname: gw_addr.ip().to_string(),
            port: gw_addr.port(),
            use_tls: false,
            auth: easyrdp_core::profile::GatewayAuth::Token,
            resource_id: "secure".into(),
            token: Some("invalid-jwt".into()),
            ..Default::default()
        };
        let bridge = easyrdp_rdp::gateway_bridge::start_bridge(&gw, "127.0.0.1", backend.port())
            .await
            .unwrap();
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", bridge.local_port()))
            .await
            .unwrap();
        // 写入后等对端关闭（WS 401 → 桥放弃 → TCP FIN）；读到 0/错误即预期
        sock.write_all(b"probe").await.unwrap();
        let mut buf = [0u8; 16];
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), sock.read(&mut buf)).await;
        match result {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {} // 连接被断开 = 预期
            Ok(Ok(n)) => panic!("无效 token 不应收到回显数据 (收到 {n} 字节)"),
        }
        drop(bridge);
    }
}
