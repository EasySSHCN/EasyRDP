//! 端口敲门客户端（knockd 序列敲门 / fwknop SPA 单包授权兼容）。
//!
//! 仅客户端：RDP 连接建立前向目标服务器发送敲门包/SPA 包以触发服务器端
//! 防火墙临时放行默认端口 tcp/3389（不改服务器）。
//!
//! 组成：
//! - [`KnockConfig`]：随 SessionProfile 持久化的敲门配置（serde camelCase，
//!   兼容旧 JSON——缺字段全部走 serde default）。
//! - [`knock_sequence`]：knockd 兼容序列敲门（UDP sendto / TCP SYN 连接后
//!   立即断开），包间隔 delayMs，全部发包须在 seqTimeout 秒内完成。
//! - [`knock_spa`]：SPA 单包授权（libfko 构造原生线格式包，可选 HMAC-SHA256）。
//! - [`poll_port`]：敲门完成后轮询 tcp/3389 可达性（500ms × 8 次）。
//!
//! ## SPA 实现（R-20261122-SPA 互通性改造）
//!
//! SPA 包由 **fwknop 官方 C 库 libfko 2.6.10**（vendor 于
//! `vendor/libfko-2.6.10/`，经 build.rs + cc 直接编入二进制）构造，Rust 侧
//! 仅做最小 FFI（[`crate::fko`]），与原生 fwknopd 逐字节互通（真机回归见
//! WSL fwknopd 2.6.10 验证记录）。密钥语义为 fwknop 原生裸密钥：
//! * `key`：Rijndael-256 加密密钥字符串（libfko 内部 PBKDF2 派生），
//!   与 `fwknop --key-gen` / access.conf `KEY`/`KEY_BASE64` 兼容；
//! * `hmacKey`：HMAC-SHA256 密钥字符串，与 `HMAC_KEY`/`HMAC_KEY_BASE64` 兼容。
//!
//! 线格式：`<b64(iv+Rijndael-256-CBC密文)>[:<b64(HMAC-SHA256)>]`。
//!
//! ⚠ **密钥语义变化**：旧版自研 SPA（SHA256(key) 派生 AES-256-CBC、自定义
//! 明文字段、`port,proto` 消息格式）与 fwknop 不互通，已整体移除。旧配置
//! 中保存的 SPA 密钥按旧语义生成，**升级后必须按 fwknop --key-gen 语义
//! 重新生成密钥**（明文密钥可直接填；fwknop 生成的 base64 密钥需填
//! base64 解码后的原文或按服务器 access.conf 一致使用 KEY_BASE64 形态）。
//!
//! ## 防 shell 注入
//!
//! 本模块只做网络发包，不执行任何命令；`preConnectCommand` 由调用方按
//! 平台 shell 语义直接传给 spawn（见 src-tauri session.rs），配置导入
//! 路径不做任何字符串拼接/二次解释。

use crate::fko;
use serde::{Deserialize, Serialize};
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// 敲门类型：sequence = knockd 序列敲门；spa = fwknop 单包授权。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnockType {
    Sequence,
    Spa,
}

/// 序列敲门包协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnockProtocol {
    Udp,
    Tcp,
}

impl KnockProtocol {
    fn as_str(&self) -> &'static str {
        match self {
            KnockProtocol::Udp => "udp",
            KnockProtocol::Tcp => "tcp",
        }
    }
}

/// 端口敲门配置（SessionProfile.knock；serde camelCase 对齐产品 JSON 约定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KnockConfig {
    /// 是否启用敲门。false = 完全保持既有直连行为。
    pub enabled: bool,
    /// sequence | spa
    #[serde(rename = "type")]
    pub knock_type: KnockType,
    /// 序列敲门端口序列（knockd 顺序敏感）。
    pub sequence: Vec<u16>,
    /// 序列敲门包协议 udp|tcp（默认 udp）。
    pub protocol: KnockProtocol,
    /// 序列敲门总时限（秒）：全部包必须在该时限内发出。
    pub seq_timeout: u64,
    /// 相邻两包间隔（毫秒）。
    pub delay_ms: u64,
    /// SPA 端口（fwknopd 默认 UDP 62201）。
    pub spa_port: u16,
    /// SPA 加密密钥（fwknop 裸密钥语义；存储态经 DPAPI 加密，见 protect/unprotect）。
    pub key: String,
    /// SPA HMAC-SHA256 密钥（fwknop 裸密钥语义；可选；非空时启用尾签）。
    pub hmac_key: String,
    /// 请求放行的端口列表（默认 tcp/3389；格式 `tcp/3389[,udp/53...]`）。
    pub open_ports: String,
    /// SPA 开门有效期（秒，服务器侧 fwTimeout 语义；客户端仅用于提示/去重窗口）。
    pub fw_timeout: u64,
}

impl Default for KnockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            knock_type: KnockType::Sequence,
            sequence: Vec::new(),
            protocol: KnockProtocol::Udp,
            seq_timeout: 10,
            delay_ms: 150,
            spa_port: 62201,
            key: String::new(),
            hmac_key: String::new(),
            open_ports: "tcp/3389".to_string(),
            fw_timeout: 30,
        }
    }
}

/// 敲门失败分类（错误信息含机器可读码，前端做分类文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnockError {
    /// 配置非法 / 密钥缺失 / 包构造失败（检查密钥/配置）。
    Invalid(String),
    /// 包已发出但开门端口始终不可达（超时）。
    Timeout(String),
}

impl KnockError {
    /// 机器可读码：knock_invalid / knock_timeout（前端错误分类依据）。
    pub fn code(&self) -> &'static str {
        match self {
            KnockError::Invalid(_) => "knock_invalid",
            KnockError::Timeout(_) => "knock_timeout",
        }
    }
}

impl std::fmt::Display for KnockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KnockError::Invalid(m) => write!(f, "knock_invalid: {m}"),
            KnockError::Timeout(m) => write!(f, "knock_timeout: {m}"),
        }
    }
}

/// SPA 阶段输出（开门提示/去重窗口用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnockOutcome {
    /// 敲门完成时刻（用于日志）。
    pub at: Instant,
    /// 建议的重试去重窗口（秒）：sequence = seq_timeout；spa = fw_timeout。
    pub dedup_secs: u64,
}

impl KnockConfig {
    /// 模型级校验：序列端口合法 / SPA 密钥非空 / 端口范围 1-65535。
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        if !self.enabled {
            return Ok(());
        }
        for (i, p) in self.sequence.iter().enumerate() {
            if *p == 0 {
                errs.push(format!("knock.sequence[{}] 端口非法: {} (须为 1-65535)", i, p));
            }
        }
        if self.knock_type == KnockType::Sequence && self.enabled && self.sequence.is_empty() {
            errs.push("敲门已启用但 sequence 序列为空".to_string());
        }
        if self.knock_type == KnockType::Spa {
            if self.key.trim().is_empty() {
                errs.push("SPA 需要填写加密密钥（key）".to_string());
            }
            if self.spa_port == 0 {
                errs.push(format!("knock.spaPort 非法: {} (须为 1-65535)", self.spa_port));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// 落盘前加密 SPA 密钥（DPAPI；幂等；复用网关凭据同一 secret 层）。
    pub fn protect_secrets(&mut self) -> Result<(), String> {
        fn enc(v: &mut String) -> Result<(), String> {
            if !v.is_empty() && !crate::secret::is_protected_format(v) {
                *v = crate::secret::protect(v)?;
            }
            Ok(())
        }
        enc(&mut self.key)?;
        enc(&mut self.hmac_key)?;
        Ok(())
    }

    /// 读盘后解密 SPA 密钥（运行时路径；失败保留原值并返回错误，上层降级）。
    pub fn unprotect_secrets(&mut self) -> Result<(), String> {
        fn dec(v: &mut String) -> Result<(), String> {
            if !v.is_empty() && crate::secret::is_protected_format(v) {
                *v = crate::secret::unprotect(v)?;
            }
            Ok(())
        }
        dec(&mut self.key)?;
        dec(&mut self.hmac_key)?;
        Ok(())
    }
}

/// 解析 openPorts 字符串（如 `tcp/3389,udp/53`）→ (proto, port) 列表。
fn parse_open_ports(s: &str) -> Result<Vec<(String, u16)>, String> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (proto, port) = part
            .split_once('/')
            .ok_or_else(|| format!("openPorts 段非法（应为 proto/port）: {part}"))?;
        let proto = proto.trim().to_ascii_lowercase();
        if proto != "tcp" && proto != "udp" {
            return Err(format!("openPorts 协议非法（tcp|udp）: {proto}"));
        }
        let port: u16 = port
            .trim()
            .parse()
            .map_err(|_| format!("openPorts 端口非法: {port}"))?;
        if port == 0 {
            return Err(format!("openPorts 端口非法: {port}"));
        }
        out.push((proto, port));
    }
    if out.is_empty() {
        return Err("openPorts 为空".to_string());
    }
    Ok(out)
}

/// 执行敲门总入口：按配置分发到 sequence / SPA，随后轮询开门端口。
/// 返回（轮询结果, 敲门输出）；失败返回分类错误（knock_invalid/knock_timeout）。
pub fn perform_knock(host: &str, cfg: &KnockConfig) -> Result<KnockOutcome, KnockError> {
    if !cfg.enabled {
        return Ok(KnockOutcome { at: Instant::now(), dedup_secs: 0 });
    }
    let outcome = match cfg.knock_type {
        KnockType::Sequence => knock_sequence(host, cfg)?,
        KnockType::Spa => knock_spa(host, cfg)?,
    };
    // 开门端口：openPorts 第一段（默认 tcp/3389）。
    let (proto, port) = parse_open_ports(&cfg.open_ports)
        .map_err(KnockError::Invalid)?
        .into_iter()
        .next()
        .ok_or_else(|| KnockError::Invalid("openPorts 为空".into()))?;
    if proto != "tcp" {
        return Err(KnockError::Invalid(format!(
            "开门端口须为 tcp 才可轮询可达性，实际 {proto}/{port}"
        )));
    }
    if poll_port(host, port) {
        Ok(outcome)
    } else {
        Err(KnockError::Timeout(format!(
            "敲门包已发送，但 {host}:{port} 在轮询窗口内始终不可达"
        )))
    }
}

/// knockd 兼容序列敲门：按序发包（UDP sendto / TCP SYN 即断），
/// 包间隔 delay_ms，全部发包须在 seq_timeout 秒内完成。
fn knock_sequence(host: &str, cfg: &KnockConfig) -> Result<KnockOutcome, KnockError> {
    cfg.validate().map_err(|errs| KnockError::Invalid(errs.join("; ")))?;
    let deadline = Instant::now() + Duration::from_secs(cfg.seq_timeout.max(1));
    let sock = if cfg.protocol == KnockProtocol::Udp {
        Some(UdpSocket::bind("0.0.0.0:0").map_err(|e| KnockError::Invalid(format!("UDP socket 创建失败: {e}")))?)
    } else {
        None
    };
    for (i, port) in cfg.sequence.iter().enumerate() {
        if Instant::now() >= deadline {
            return Err(KnockError::Timeout(format!(
                "seqTimeout({}s) 内未发完 {} 个敲门包（停在 {}）",
                cfg.seq_timeout,
                cfg.sequence.len(),
                i
            )));
        }
        let target = resolve(host, *port).map_err(KnockError::Invalid)?;
        match &sock {
            Some(s) => {
                // 0~8 字节随机负载（knockd 只看端口，不校验内容）。
                let n = rand_byte() as usize % 9;
                let payload = random_bytes(n);
                s.send_to(&payload, target)
                    .map_err(|e| KnockError::Invalid(format!("UDP sendto {port} 失败: {e}")))?;
            }
            None => {
                // TCP SYN：connect 成功后立即断开（RST 正常，服务器 knockd 只看 SYN）。
                let _ = TcpStream::connect_timeout(&target, Duration::from_millis(1500));
            }
        }
        if i + 1 < cfg.sequence.len() {
            std::thread::sleep(Duration::from_millis(cfg.delay_ms.min(2000)));
        }
    }
    Ok(KnockOutcome { at: Instant::now(), dedup_secs: cfg.seq_timeout + 10 })
}

/// fwknop 风格 SPA：libfko 构造原生包 → UDP 发送。
fn knock_spa(host: &str, cfg: &KnockConfig) -> Result<KnockOutcome, KnockError> {
    cfg.validate().map_err(|errs| KnockError::Invalid(errs.join("; ")))?;
    let packet = build_spa_packet(host, cfg).map_err(KnockError::Invalid)?;
    let target = resolve(host, cfg.spa_port).map_err(KnockError::Invalid)?;
    let sock = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| KnockError::Invalid(format!("UDP socket 创建失败: {e}")))?;
    sock.send_to(&packet, target)
        .map_err(|e| KnockError::Invalid(format!("SPA 包发送失败: {e}")))?;
    Ok(KnockOutcome { at: Instant::now(), dedup_secs: cfg.fw_timeout.max(10) })
}

/// 解密存储态密钥：已是加密格式 → 解密；明文 → 原样返回（测试/兼容路径）。
fn decode_secret(v: &str) -> Result<String, String> {
    if v.is_empty() {
        return Ok(String::new());
    }
    if crate::secret::is_protected_format(v) {
        crate::secret::unprotect(v)
    } else {
        Ok(v.to_string())
    }
}

/// 真机回归入口（examples/spa_send.rs 用）：暴露与生产路径完全一致的
/// SPA 包构造（`build_spa_packet`），不做 DPAPI 解密路径分支。
pub fn example_build_packet(host: &str, cfg: &KnockConfig) -> Result<Vec<u8>, String> {
    build_spa_packet(host, cfg)
}

/// 构造 fwknop 原生线格式 SPA 包（libfko 路径，见模块注释）。
///
/// 访问消息按 fwknop 语义组装：`<allow_ip>,<proto>/<port>[,...]`；
/// allow_ip 用 SPA 服务器地址（客户端未知源 IP 时 fwknop 约定填服务器地址）。
fn build_spa_packet(host: &str, cfg: &KnockConfig) -> Result<Vec<u8>, String> {
    let key_plain = decode_secret(&cfg.key)?;
    if key_plain.is_empty() {
        return Err("SPA 密钥为空（检查密钥配置）".into());
    }
    let hmac_plain = decode_secret(&cfg.hmac_key)?;
    let ports = parse_open_ports(&cfg.open_ports)?;
    // `<allow_ip>,tcp/3389[,udp/53...]`
    let mut message = host.trim().to_string();
    for (proto, port) in &ports {
        message.push(',');
        message.push_str(proto);
        message.push('/');
        message.push_str(&port.to_string());
    }
    fko::spa_data_final_native(
        &message,
        key_plain.as_bytes(),
        hmac_plain.as_bytes(),
    )
}

/// 敲门后轮询开门端口可达性：500ms × 最多 8 次（每次连接超时 1200ms）。
pub fn poll_port(host: &str, port: u16) -> bool {
    let addr = match resolve(host, port) {
        Ok(a) => a,
        Err(_) => return false,
    };
    for _ in 0..8 {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(1200)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS 解析失败 {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("DNS 解析无结果 {host}:{port}"))
}

// ---------------------------------------------------------------------------
// 随机数（序列敲门负载用；SPA 包随机数由 libfko 内部生成）
// ---------------------------------------------------------------------------

fn random_bytes(n: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// 单字节随机（序列敲门负载长度用；低安全要求）。
fn rand_byte() -> u8 {
    random_bytes(1)[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------- SPA 包格式（libfko 原生线格式） ----------------

    #[test]
    fn spa_packet_native_wire_format() {
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            spa_port: 62201,
            key: "spa-secret-key".into(),
            hmac_key: "hmac-secret".into(),
            open_ports: "tcp/3389".into(),
            ..KnockConfig::default()
        };
        let packet = build_spa_packet("10.0.0.5", &cfg).expect("构造 SPA 包");
        let s = String::from_utf8(packet.clone()).expect("线格式应为 ASCII");
        // 原生格式：b64密文 与 b64hmac 直接拼接（无 ':'，无 '=' 填充）。
        assert!(!s.contains(':'), "原生线格式不含 ':': {s}");
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
            "须为无填充标准 base64: {s}");
        // 相同输入（libfko 内部随机 IV）包应不同但长度稳定。
        let packet2 = build_spa_packet("10.0.0.5", &cfg).unwrap();
        assert_ne!(packet, packet2, "两次构造应因随机 IV/HMAC 不同而不同");
        assert_eq!(packet.len(), packet2.len(), "包长度应稳定");
    }

    #[test]
    fn spa_empty_key_is_invalid() {
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            key: String::new(),
            ..KnockConfig::default()
        };
        let err = perform_knock("127.0.0.1", &cfg).unwrap_err();
        assert_eq!(err.code(), "knock_invalid", "{err}");
    }

    #[test]
    fn spa_bad_open_ports_is_invalid() {
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            key: "k".into(),
            open_ports: "icmp/7".into(),
            ..KnockConfig::default()
        };
        let err = perform_knock("127.0.0.1", &cfg).unwrap_err();
        assert_eq!(err.code(), "knock_invalid", "{err}");
    }

    #[test]
    fn spa_no_hmac_single_segment() {
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            key: "spa-secret-key".into(),
            hmac_key: String::new(),
            open_ports: "tcp/3389".into(),
            ..KnockConfig::default()
        };
        let packet = build_spa_packet("10.0.0.5", &cfg).unwrap();
        let s = String::from_utf8(packet).unwrap();
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
            "无 HMAC 时仍为无填充 base64 单段: {s}");
    }

    // ---------------- 序列敲门 / openPorts 解析 ----------------

    #[test]
    fn parse_open_ports_ok_and_errors() {
        let ports = parse_open_ports("tcp/3389, udp/53").unwrap();
        assert_eq!(ports, vec![("tcp".to_string(), 3389), ("udp".to_string(), 53)]);
        assert!(parse_open_ports("icmp/7").is_err());
        assert!(parse_open_ports("tcp/0").is_err());
        assert!(parse_open_ports("3389").is_err());
        assert!(parse_open_ports("").is_err());
    }

    #[test]
    fn sequence_empty_with_enabled_is_invalid() {
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Sequence,
            sequence: vec![],
            ..KnockConfig::default()
        };
        let err = perform_knock("127.0.0.1", &cfg).unwrap_err();
        assert_eq!(err.code(), "knock_invalid", "{err}");
    }

    /// 本地 UDP 监听：SPA 包真实发出（端到端解密验证由 WSL fwknopd
    /// 真机回归完成，见需求池登记）。
    #[test]
    fn spa_send_local_udp_listener() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            spa_port: port,
            key: "listener-key".into(),
            hmac_key: "listener-hmac".into(),
            open_ports: "tcp/3389".into(),
            ..KnockConfig::default()
        };
        let packet = build_spa_packet("127.0.0.1", &cfg).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = resolve("127.0.0.1", port).unwrap();
        sock.send_to(&packet, target).unwrap();
        let mut buf = vec![0u8; 4096];
        let (n, _) = listener.recv_from(&mut buf).unwrap();
        let received = String::from_utf8(buf[..n].to_vec()).unwrap();
        assert_eq!(received, String::from_utf8(packet).unwrap());
        assert!(!received.contains(':'), "原生线格式不含 ':'");
    }

    /// 序列敲门（UDP）发包真实发出：本地监听逐一收到对应端口序列。
    #[test]
    fn sequence_udp_local_receivers() {
        let mut socks = Vec::new();
        let mut ports = Vec::new();
        for _ in 0..3 {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            ports.push(s.local_addr().unwrap().port());
            socks.push(s);
        }
        let cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Sequence,
            sequence: ports.clone(),
            protocol: KnockProtocol::Udp,
            delay_ms: 10,
            seq_timeout: 5,
            open_ports: format!("tcp/{}", ports[0]), // 轮询第一端口必失败 → Timeout
            ..KnockConfig::default()
        };
        // 后台线程收包；主线程执行（poll_port 对随机本地端口必失败 = knock_timeout）。
        let receiver = std::thread::spawn(move || {
            for s in &socks {
                let mut buf = [0u8; 64];
                s.recv_from(&mut buf).unwrap();
            }
        });
        let err = perform_knock("127.0.0.1", &cfg).unwrap_err();
        receiver.join().unwrap();
        assert_eq!(err.code(), "knock_timeout", "{err}");
    }

    /// 敲门配置 serde camelCase + 旧 JSON 兼容（缺字段走 default）。
    #[test]
    fn knock_config_serde_compat() {
        let old = r#"{ "enabled": true, "type": "sequence", "sequence": [7000, 8000, 9000] }"#;
        let cfg: KnockConfig = serde_json::from_str(old).unwrap();
        assert_eq!(cfg.protocol, KnockProtocol::Udp);
        assert_eq!(cfg.seq_timeout, 10);
        assert_eq!(cfg.delay_ms, 150);
        assert_eq!(cfg.spa_port, 62201);
        assert_eq!(cfg.open_ports, "tcp/3389");
        assert_eq!(cfg.fw_timeout, 30);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"seqTimeout\""), "{json}");
        assert!(json.contains("\"delayMs\""), "{json}");
        let back: KnockConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn knock_secrets_protect_roundtrip() {
        let mut cfg = KnockConfig {
            enabled: true,
            knock_type: KnockType::Spa,
            key: "plain-key-🔐".into(),
            hmac_key: "plain-hmac".into(),
            ..KnockConfig::default()
        };
        cfg.protect_secrets().unwrap();
        let stored = serde_json::to_string(&cfg).unwrap();
        assert!(!stored.contains("plain-key"), "落盘串不得含明文 key");
        assert!(!stored.contains("plain-hmac"), "落盘串不得含明文 hmac");
        // 幂等
        cfg.protect_secrets().unwrap();
        cfg.unprotect_secrets().unwrap();
        assert_eq!(cfg.key, "plain-key-🔐");
        assert_eq!(cfg.hmac_key, "plain-hmac");
    }
}
