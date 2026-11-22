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

    /// e2e 专用宽松策略：`*` × `*` × connect/list 全 allow
    /// （默认矩阵 deny by default 会拒绝测试用户的 connect）。
    fn permissive_policy() -> crate::policy::PolicyHandle {
        let pc = crate::policy::PolicyConfig {
            rules: vec![
                crate::policy::PolicyRuleConfig {
                    resource: "*".into(),
                    role: "*".into(),
                    action: "connect".into(),
                    decision: "allow".into(),
                },
                crate::policy::PolicyRuleConfig {
                    resource: "*".into(),
                    role: "*".into(),
                    action: "list".into(),
                    decision: "allow".into(),
                },
            ],
        };
        crate::policy::PolicyHandle::from_matrix(
            crate::policy::PolicyMatrix::from_config(&pc).expect("compile permissive matrix"),
        )
    }

    /// 启动 axum gateway（auth + tunnel 路由）在 127.0.0.1 临时端口，
    /// 返回 (监听地址, AppState)——state 供测试直接签发 JWT。
    async fn spawn_gateway(registry: ResourceRegistry) -> (std::net::SocketAddr, Arc<AppState>) {
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: registry,
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: None,
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

        // 4) 客户端桥接模块（与 GUI connect 同一入口，async start_bridge）
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
        // 覆盖多帧场景：16KiB（桥泵帧切分阈值）+ 二进制任意字节（含 0x00/0xFF）
        let pattern: Vec<u8> = (0..48 * 1024u32).map(|i| (i % 251) as u8).collect();
        sock.write_all(&pattern).await.expect("write pattern");

        let mut got = vec![0u8; pattern.len()];
        sock.read_exact(&mut got).await.expect("read echo");
        assert_eq!(got, pattern, "经网关 WS 隧道回读的字节流必须与写入一致");

        // 6) 半关闭：echo EOF 后泵收尾链不挂起
        sock.shutdown().await.expect("shutdown");

        drop(bridge); // 关停回环桥
    }

    /// 反向流：后端先推数据（服务端主动包）经隧道到达客户端。
    #[tokio::test]
    async fn tunnel_backend_push_reaches_client() {
        // 后端：accept 后主动推一段服务端横幅
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
        // 客户端读到横幅（read_exact 长度 = banner 长度）
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

    // -----------------------------------------------------------------
    // F4-V1（R-20261118-012）：wss 回环集成测试
    // -----------------------------------------------------------------

    /// 运行时生成自签证书（SAN=localhost，rcgen，零外部依赖），
    /// 直接返回 (cert PEM, key PEM)。不落盘：早期用 `temp_dir()+纳秒时间戳`
    /// 写临时文件，Windows SystemTime 粒度粗（约毫秒级），两个 wss 测试并行
    /// 启动会得到相同目录名，后写的测试用另一对密钥覆盖同一目录，
    /// 先启动的加载方读到「A 的证书 + B 的私钥」→ InconsistentKeys(KeyMismatch)。
    /// axum-server 0.7 的 RustlsConfig::from_pem 支持内存 PEM，直接绕开文件竞态。
    fn self_signed_cert_pem() -> (String, String) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("generate self-signed cert");
        (ck.cert.pem(), ck.key_pair.serialize_pem())
    }

    /// 启动 HTTPS/WSS gateway（axum-server rustls，自签证书）。
    /// 端口策略：先 tokio 绑 :0 取端口再释放，然后 axum-server 绑同端口
    /// （本机回环 TOCTOU 窗口极小，测试可接受）。
    async fn spawn_gateway_tls(registry: ResourceRegistry, cert_pem: &str, key_pem: &str) -> std::net::SocketAddr {
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: registry,
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: None,
        });
        let app = axum::Router::new()
            .merge(crate::auth::router(Arc::clone(&state)))
            .merge(tunnel::router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let tls_cfg =
            axum_server::tls_rustls::RustlsConfig::from_pem(
                cert_pem.as_bytes().to_vec(),
                key_pem.as_bytes().to_vec(),
            )
            .await
            .expect("load tls config");
        tokio::spawn(async move {
            axum_server::bind_rustls(addr, tls_cfg)
                .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .expect("gateway https serve");
        });
        addr
    }

    /// wss 回环：自签证书 HTTPS gateway + 客户端桥（use_tls + allow_self_signed），
    /// 全链路 TLS 隧道 echo 字节一致。
    #[tokio::test]
    async fn wss_tunnel_echo_end_to_end_with_self_signed_cert() {
        let backend = spawn_echo_backend().await;
        let registry = ResourceRegistry::default();
        registry.register("echo", backend.to_string());
        let (cert, key) = self_signed_cert_pem();
        let gw_addr = spawn_gateway_tls(registry, &cert, &key).await;

        // 客户端桥：wss://localhost:<port>/tunnel/echo，allow_self_signed=true
        // （自签证书需客户端 UI 明示勾选放行；默认 false 时 SChannel 会拒绝）
        let gw = easyrdp_core::profile::GatewayConfig {
            hostname: "localhost".into(), // 证书 SAN=localhost
            port: gw_addr.port(),
            use_tls: true,
            auth: easyrdp_core::profile::GatewayAuth::Token,
            resource_id: "echo".into(),
            token: Some(
                AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL)
                    .issue_token("alice", &["user"])
                    .unwrap(),
            ),
            allow_self_signed: true,
            ..Default::default()
        };
        let bridge = easyrdp_rdp::gateway_bridge::start_bridge(&gw, "127.0.0.1", backend.port())
            .await
            .expect("wss bridge spawn");

        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", bridge.local_port()))
            .await
            .expect("connect bridge");
        let pattern: Vec<u8> = (0..48 * 1024u32).map(|i| (i % 251) as u8).collect();
        sock.write_all(&pattern).await.expect("write pattern");
        let mut got = vec![0u8; pattern.len()];
        tokio::time::timeout(std::time::Duration::from_secs(10), sock.read_exact(&mut got))
            .await
            .expect("wss echo within 10s")
            .expect("read echo");
        assert_eq!(got, pattern, "wss 隧道回读字节流必须与写入一致");
        drop(sock);
        drop(bridge);
    }

    /// allow_self_signed=false（默认）：自签证书应被客户端证书校验拒绝。
    /// 预检（本测试用登录端点不触发——Token 直用，TLS 握手发生在 WS 连接期，
    /// 故以桥回环连接失败/无回显断言）。
    #[tokio::test]
    async fn wss_rejects_self_signed_when_validation_enabled() {
        let backend = spawn_echo_backend().await;
        let registry = ResourceRegistry::default();
        registry.register("echo", backend.to_string());
        let (cert, key) = self_signed_cert_pem();
        let gw_addr = spawn_gateway_tls(registry, &cert, &key).await;

        let gw = easyrdp_core::profile::GatewayConfig {
            hostname: "localhost".into(),
            port: gw_addr.port(),
            use_tls: true,
            auth: easyrdp_core::profile::GatewayAuth::Token,
            resource_id: "echo".into(),
            token: Some(
                AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL)
                    .issue_token("alice", &["user"])
                    .unwrap(),
            ),
            allow_self_signed: false,
            ..Default::default()
        };
        let bridge = easyrdp_rdp::gateway_bridge::start_bridge(&gw, "127.0.0.1", backend.port())
            .await
            .expect("bridge spawn succeeds (pre-flight token only)");
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", bridge.local_port()))
            .await
            .unwrap();
        sock.write_all(b"probe").await.unwrap();
        let mut buf = [0u8; 16];
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(10), sock.read(&mut buf)).await;
        match result {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {} // TLS 校验失败断开 = 预期
            Ok(Ok(n)) => panic!("自签证书在严格校验下不应回显数据 (收到 {n} 字节)"),
        }
        drop(sock);
        drop(bridge);
    }

    /// OTP 二次认证（HTTP 层）：配置 totp_secret 的用户，缺 mfa_code 得
    /// 401 "otp_required"；带正确码 → 200 换到 JWT。
    #[tokio::test]
    async fn login_otp_required_then_success_with_code() {
        use axum::http::Request;
        use tower::ServiceExt; // oneshot

        let secret = "JBSWY3DPEHPK3PXP";
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::default(),
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("alice".to_string(), secret.to_string());
                m
            },
            oidc: None,
        });
        let app = axum::Router::new()
            .merge(crate::auth::router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));

        // 1) 缺 OTP → 401 body 含 otp_required
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(r#"{"oidc_code":"alice"}"#.to_string())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("otp_required"));

        // 2) 错误码 → 401 otp_required
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(r#"{"oidc_code":"alice","mfa_code":"000000"}"#.to_string())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

        // 3) 正确 TOTP 码 → 200 + token
        let code = crate::auth::totp_code_at(secret, std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(format!(r#"{{"oidc_code":"alice","mfa_code":"{code}"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
