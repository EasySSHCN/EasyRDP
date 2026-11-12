//! 资源注册表模块：内网 RDP 资源目录 + 后端地址解析 + SSRF 防护。
//!
//! 后端地址解析优先级：
//! 1. `config.toml` 的 `[[resources]]` 表：`resource_id -> "host:port"`；
//! 2. resource_id 本身是显式 `"host:port"` 格式，且主机通过
//!    `is_allowed_backend` 私网白名单校验（防止 SSRF 任意探测公网/DNS 目标）。
//!
//! 白名单：10.0.0.0/8、172.16.0.0/12、192.168.0.0/16、
//! localhost 与 127.0.0.0/8（IPv4 回环）、::1（IPv6 回环）。
//! DNS 主机名一律拒绝：网关不代查域名，显式 IP 才允许。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::RwLock;

/// config.toml `[[resources]]` 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// 资源 ID（客户端 /tunnel/{resource_id} 使用的标识）
    pub id: String,
    /// 展示名称
    pub name: String,
    /// 后端地址 "host:port"（仅注册表内部可见，不通过 API 暴露）
    pub backend: String,
}

/// 对外资源条目（GET /api/resources）：只含 id/name，不含后端地址。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub name: String,
}

/// 资源注册表：id -> backend。
/// 骨架阶段启动时由 config.toml 一次性加载；用 RwLock 便于后续动态注册。
#[derive(Default)]
pub struct ResourceRegistry {
    map: RwLock<HashMap<String, String>>,
}

impl ResourceRegistry {
    pub fn from_configs(configs: &[ResourceConfig]) -> Self {
        let map = configs
            .iter()
            .map(|r| (r.id.clone(), r.backend.clone()))
            .collect();
        Self {
            map: RwLock::new(map),
        }
    }

    pub fn register(&self, id: impl Into<String>, backend: impl Into<String>) {
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(id.into(), backend.into());
    }

    /// 列出对外资源信息（id/name，不含后端地址）。
    pub fn list(&self) -> Vec<ResourceInfo> {
        self.map
            .read()
            .expect("registry mutex poisoned")
            .keys()
            .map(|id| ResourceInfo {
                id: id.clone(),
                name: id.clone(),
            })
            .collect()
    }

    /// 依据 resource_id 解析后端 "host:port"：
    /// 先查注册表；否则接受显式 "host:port" 且主机在私网白名单内。
    pub fn resolve_backend(&self, resource_id: &str) -> Option<String> {
        let lookup = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .get(resource_id)
            .cloned();

        if let Some(backend) = lookup {
            return if parse_host_port(&backend).is_some() {
                Some(backend)
            } else {
                None
            };
        }

        // 显式 "host:port" 直连：必须格式合法且主机在私网白名单内。
        let (host, port) = parse_host_port(resource_id)?;
        if !is_allowed_backend(host) {
            return None;
        }
        Some(format!("{host}:{port}"))
    }
}

/// 解析 "host:port"，返回 (host, port)。host 可为 IP 字面量或 localhost。
pub fn parse_host_port(addr: &str) -> Option<(&str, u16)> {
    let (host, port) = addr.trim().rsplit_once(':')?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    Some((host, port))
}

/// SSRF 防护白名单：仅允许私网 IPv4 段与回环（localhost / 127.0.0.0/8 / ::1）。
///
/// 规则：
/// - "localhost"（忽略大小写）允许；
/// - 含任何字母的主机串（即 DNS 域名）一律拒绝；
/// - IP 字面量：IPv4 须落在 10/8、172.16/12、192.168/16 或回环 127/8；
///   IPv6 仅允许回环 ::1。
pub fn is_allowed_backend(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // 拒绝主机名/DNS 域名：仅允许 IP 字面量（IPv6 字面量同样含冒号之外的字母时被拒，
    // 仅十六进制形式的 IPv6 地址可继续走解析）。
    if host.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) || a == 127
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_backends_include_private_ranges() {
        for host in [
            "10.0.0.5",
            "10.255.255.255",
            "192.168.1.10",
            "192.168.0.0",
            "172.16.0.1",
            "172.31.255.254",
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
        ] {
            assert!(is_allowed_backend(host), "{host} 应被允许");
        }
    }

    #[test]
    fn disallowed_backends_reject_public_and_dns() {
        for host in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",     // 恰好超出 172.16-31
            "172.15.0.1",     // 低于 172.16
            "example.com",    // DNS 域名
            "rdp.internal",   // 内网域名也不放行（不代查 DNS）
            "",               // 空
            "10.0.0.5:3389",  // 带端口的串不是主机
            "[::1]:3389",     // 带端口/括号不是主机
            "0.0.0.0",
            "169.254.1.1",
            "::1",            // IPv6 回环按字面量解析允许
            "2001:db8::1",    // 含字母（db8）且非回环 -> 拒绝
        ] {
            assert_eq!(
                is_allowed_backend(host),
                host == "::1",
                "{host} 判定不符合预期"
            );
        }
    }

    #[test]
    fn parse_host_port_requires_explicit_format() {
        assert_eq!(parse_host_port("10.0.0.5:3389"), Some(("10.0.0.5", 3389)));
        assert_eq!(parse_host_port(" localhost :3389 "), Some(("localhost", 3389)));
        assert_eq!(parse_host_port("10.0.0.5"), None); // 缺端口
        assert_eq!(parse_host_port("10.0.0.5:notaport"), None);
        assert_eq!(parse_host_port("10.0.0.5:0"), None); // 端口 0 无效
        assert_eq!(parse_host_port(":3389"), None); // 缺主机
    }

    #[test]
    fn registry_resolves_configured_resource() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "rdp-win01".into(),
            name: "Windows 跳板".into(),
            backend: "10.0.0.5:3389".into(),
        }]);
        assert_eq!(
            reg.resolve_backend("rdp-win01").as_deref(),
            Some("10.0.0.5:3389")
        );
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn explicit_host_port_resolved_only_when_allowed() {
        let reg = ResourceRegistry::default();
        // 显式私网 host:port 允许直连
        assert_eq!(
            reg.resolve_backend("192.168.1.20:3389").as_deref(),
            Some("192.168.1.20:3389")
        );
        // 公网 / DNS 一律拒绝（SSRF 防护）
        assert_eq!(reg.resolve_backend("8.8.8.8:3389"), None);
        assert_eq!(reg.resolve_backend("evil.example.com:3389"), None);
        assert_eq!(reg.resolve_backend("not-a-addr"), None);
    }

    #[test]
    fn registry_backend_must_also_be_valid_host_port() {
        let reg = ResourceRegistry::default();
        reg.register("bad", "10.0.0.5"); // 注册表里缺端口 -> 解析失败
        assert_eq!(reg.resolve_backend("bad"), None);
        reg.register("ok", "10.0.0.6:3389");
        assert!(reg.resolve_backend("ok").is_some());
    }
}
