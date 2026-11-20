//! 资源注册表模块：内网 RDP 资源目录 + 后端地址解析 + 角色级授权 + SSRF 防护。
//!
//! 后端地址解析优先级：
//! 1. `config.toml` 的 `[[resources]]` 表：`resource_id -> "host:port"`，
//!    并按该资源可选的 `allowed_roles` 做 JWT claims.roles 交集授权
//!    （缺省 = 所有角色可用；无交集返回 `BackendResolution::Forbidden`）；
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
    /// 允许访问该资源的角色列表（与 JWT claims.roles 求交集）；
    /// 缺省（None 或空数组）= 所有角色可用。
    #[serde(default)]
    pub allowed_roles: Option<Vec<String>>,
}

/// 注册表内部条目：展示名 + 后端地址 + 角色授权。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub name: String,
    pub backend: String,
    pub allowed_roles: Option<Vec<String>>,
}

/// resolve_backend 的授权判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendResolution {
    /// 授权通过，返回后端地址 "host:port"。
    Allowed(String),
    /// 资源存在但当前角色不被 allowed_roles 允许（映射 403 tunnel_forbidden）。
    Forbidden,
    /// 资源未注册 / 后端地址非法 / 显式直连未过 SSRF 白名单（映射 404）。
    NotFound,
}

/// 对外资源条目（GET /api/resources）：只含 id/name，不含后端地址。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub name: String,
}

/// 资源注册表：id -> ResourceEntry。
/// 骨架阶段启动时由 config.toml 一次性加载；用 RwLock 便于后续动态注册。
#[derive(Default)]
pub struct ResourceRegistry {
    map: RwLock<HashMap<String, ResourceEntry>>,
}

impl ResourceRegistry {
    pub fn from_configs(configs: &[ResourceConfig]) -> Self {
        let map = configs
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    ResourceEntry {
                        name: r.name.clone(),
                        backend: r.backend.clone(),
                        allowed_roles: r.allowed_roles.clone(),
                    },
                )
            })
            .collect();
        Self {
            map: RwLock::new(map),
        }
    }

    /// 简易注册（骨架/测试用）：展示名回退为 id，角色授权为缺省全放行。
    pub fn register(&self, id: impl Into<String>, backend: impl Into<String>) {
        let id = id.into();
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.clone(),
                ResourceEntry {
                    name: id,
                    backend: backend.into(),
                    allowed_roles: None,
                },
            );
    }

    /// 完整注册：展示名 + 角色授权。
    pub fn register_full(
        &self,
        id: impl Into<String>,
        name: impl Into<String>,
        backend: impl Into<String>,
        allowed_roles: Option<Vec<String>>,
    ) {
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.into(),
                ResourceEntry {
                    name: name.into(),
                    backend: backend.into(),
                    allowed_roles,
                },
            );
    }

    /// 列出对外资源信息（id/name，不含后端地址与角色授权）。
    pub fn list(&self) -> Vec<ResourceInfo> {
        self.map
            .read()
            .expect("registry mutex poisoned")
            .iter()
            .map(|(id, e)| ResourceInfo {
                id: id.clone(),
                name: e.name.clone(),
            })
            .collect()
    }

    /// 判断 claims.roles 是否被 allowed_roles 放行：
    /// allowed_roles 为 None 或空 = 所有角色可用；否则需与角色列表有交集。
    fn roles_allowed(allowed_roles: Option<&[String]>, roles: &[String]) -> bool {
        match allowed_roles {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => roles.iter().any(|r| list.contains(r)),
        }
    }

    /// 依据 resource_id 解析后端 "host:port" 并做角色级授权检查：
    /// 先查注册表（匹配 allowed_roles 才放行）；否则接受显式 "host:port"
    /// 且主机在私网白名单内（显式直连不做角色限制）。
    pub fn resolve_backend(&self, resource_id: &str, roles: &[String]) -> BackendResolution {
        let lookup = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .get(resource_id)
            .cloned();

        if let Some(entry) = lookup {
            if !Self::roles_allowed(entry.allowed_roles.as_deref(), roles) {
                return BackendResolution::Forbidden;
            }
            return if parse_host_port(&entry.backend).is_some() {
                BackendResolution::Allowed(entry.backend)
            } else {
                BackendResolution::NotFound
            };
        }

        // 显式 "host:port" 直连：必须格式合法且主机在私网白名单内。
        let Some((host, port)) = parse_host_port(resource_id) else {
            return BackendResolution::NotFound;
        };
        if !is_allowed_backend(host) {
            return BackendResolution::NotFound;
        }
        BackendResolution::Allowed(format!("{host}:{port}"))
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
            allowed_roles: None,
        }]);
        assert_eq!(
            reg.resolve_backend("rdp-win01", &[]),
            BackendResolution::Allowed("10.0.0.5:3389".into())
        );
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn list_returns_configured_names_not_id_fallback() {
        let reg = ResourceRegistry::from_configs(&[
            ResourceConfig {
                id: "rdp-win01".into(),
                name: "Windows 跳板机".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
            },
            ResourceConfig {
                id: "rdp-linux01".into(),
                name: "Linux 桌面".into(),
                backend: "192.168.1.20:3390".into(),
                allowed_roles: Some(vec!["ops".into(), "dev".into()]),
            },
        ]);
        let items = reg.list();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|r| r.id == "rdp-win01" && r.name == "Windows 跳板机"));
        assert!(items.iter().any(|r| r.id == "rdp-linux01" && r.name == "Linux 桌面"));
        // 简易注册时 name 回退为 id
        reg.register("ad-hoc", "10.0.0.9:3389");
        let ad = reg
            .list()
            .into_iter()
            .find(|r| r.id == "ad-hoc")
            .expect("ad-hoc");
        assert_eq!(ad.name, "ad-hoc");
    }

    #[test]
    fn explicit_host_port_resolved_only_when_allowed() {
        let reg = ResourceRegistry::default();
        // 显式私网 host:port 允许直连（无角色限制）
        assert_eq!(
            reg.resolve_backend("192.168.1.20:3389", &[]),
            BackendResolution::Allowed("192.168.1.20:3389".into())
        );
        // 公网 / DNS 一律拒绝（SSRF 防护）
        assert_eq!(reg.resolve_backend("8.8.8.8:3389", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("evil.example.com:3389", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("not-a-addr", &[]), BackendResolution::NotFound);
    }

    #[test]
    fn registry_backend_must_also_be_valid_host_port() {
        let reg = ResourceRegistry::default();
        reg.register("bad", "10.0.0.5"); // 注册表里缺端口 -> 解析失败
        assert_eq!(reg.resolve_backend("bad", &[]), BackendResolution::NotFound);
        reg.register("ok", "10.0.0.6:3389");
        assert!(matches!(
            reg.resolve_backend("ok", &[]),
            BackendResolution::Allowed(_)
        ));
    }

    #[test]
    fn allowed_roles_matching_role_is_permitted() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "ops-only".into(),
            name: "运维专用".into(),
            backend: "10.0.0.5:3389".into(),
            allowed_roles: Some(vec!["ops".into(), "admin".into()]),
        }]);
        assert_eq!(
            reg.resolve_backend("ops-only", &["ops".to_string(), "user".to_string()]),
            BackendResolution::Allowed("10.0.0.5:3389".into())
        );
    }

    #[test]
    fn allowed_roles_mismatch_is_forbidden() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "ops-only".into(),
            name: "运维专用".into(),
            backend: "10.0.0.5:3389".into(),
            allowed_roles: Some(vec!["ops".into(), "admin".into()]),
        }]);
        // 角色无交集 -> Forbidden（不得泄露后端地址）
        assert_eq!(
            reg.resolve_backend("ops-only", &["contractor".to_string()]),
            BackendResolution::Forbidden
        );
        assert_eq!(
            reg.resolve_backend("ops-only", &[]),
            BackendResolution::Forbidden
        );
    }

    #[test]
    fn allowed_roles_missing_or_empty_allows_every_role() {
        let reg = ResourceRegistry::from_configs(&[
            ResourceConfig {
                id: "no-roles".into(),
                name: "全放行".into(),
                backend: "10.0.0.5:3389".into(),
                allowed_roles: None,
            },
            ResourceConfig {
                id: "empty-roles".into(),
                name: "空数组等价全放行".into(),
                backend: "10.0.0.6:3389".into(),
                allowed_roles: Some(vec![]),
            },
        ]);
        for id in ["no-roles", "empty-roles"] {
            for roles in [
                vec![],
                vec!["user".to_string()],
                vec!["contractor".to_string(), "ops".to_string()],
            ] {
                assert!(
                    matches!(reg.resolve_backend(id, &roles), BackendResolution::Allowed(_)),
                    "{id} 配合角色 {roles:?} 应全放行"
                );
            }
        }
    }
}
