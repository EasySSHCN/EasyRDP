//! 策略模块（F5-V1 R-20261118-011）：可配置策略矩阵。
//!
//! 矩阵维度：资源（resource_id 或 "*"）× 角色（admin/user/readonly）×
//! 动作（connect/list）→ Allow/Deny。
//!
//! 来源：config.toml `[policy]` 段的 `[[policy.rules]]` 有序规则列表
//! （首条匹配生效；`*` 通配任一维度）。无任何规则匹配时套用默认矩阵：
//! **deny by default；admin 全通**。
//!
//! 旧版未接线的 Role/RedirectKind/Decision 重定向骨架已删除（本模块
//! 曾经的 ops/contractor check_redirect 从未被生产路径调用，且与本需求
//! 的资源×角色×动作矩阵语义冲突）。
//!
//! 热加载：[`PolicyHandle::reload_from_file`] 解析 config.toml，成功则
//! 原子换入新矩阵，失败则保留旧矩阵并返回 Err（调用方审计 warn）。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::RwLock;

/// 矩阵角色（规范化三角色；JWT claims.roles 映射见 [`canonical_role`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    User,
    ReadOnly,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::ReadOnly => "readonly",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "admin" => Some(Role::Admin),
            "user" => Some(Role::User),
            "readonly" | "read_only" | "read-only" => Some(Role::ReadOnly),
            _ => None,
        }
    }
}

/// JWT claims.roles -> 矩阵角色：含 "admin" 取 Admin（最高权限），
/// 否则显式 "readonly" 取 ReadOnly，其余（含未知角色）按 User 处理。
pub fn canonical_role(roles: &[String]) -> Role {
    if roles.iter().any(|r| r.eq_ignore_ascii_case("admin")) {
        Role::Admin
    } else if roles.iter().any(|r| r.eq_ignore_ascii_case("readonly")) {
        Role::ReadOnly
    } else {
        Role::User
    }
}

/// 矩阵动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// 建立隧道（GET /tunnel/{resource_id}）
    Connect,
    /// 资源列表（GET /api/resources）
    List,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s.trim().to_ascii_lowercase().as_str() {
            "connect" => Some(Action::Connect),
            "list" => Some(Action::List),
            _ => None,
        }
    }
}

/// 策略结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

/// config.toml `[[policy.rules]]` 条目。三个维度均可写 "*" 通配；
/// 未填字段等同 "*"。decision = "allow" | "deny"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRuleConfig {
    #[serde(default = "wildcard")]
    pub resource: String,
    #[serde(default = "wildcard")]
    pub role: String,
    #[serde(default = "wildcard")]
    pub action: String,
    pub decision: String,
}

fn wildcard() -> String {
    "*".to_string()
}

/// config.toml `[policy]` 段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub rules: Vec<PolicyRuleConfig>,
}

/// 编译后的矩阵规则（None = 通配）。
#[derive(Debug, Clone)]
struct CompiledRule {
    resource: Option<String>,
    role: Option<Role>,
    action: Option<Action>,
    allow: bool,
}

/// 策略矩阵：有序规则 + 默认结论（无规则匹配时 deny；admin 全通）。
#[derive(Debug, Clone)]
pub struct PolicyMatrix {
    rules: Vec<CompiledRule>,
}

impl PolicyMatrix {
    /// 由配置编译矩阵。解析失败的规则会导致整体失败（返回 Err），
    /// 保证热加载时坏配置不会悄悄半生效。
    pub fn from_config(cfg: &PolicyConfig) -> Result<Self, String> {
        let mut rules = Vec::with_capacity(cfg.rules.len());
        for (i, r) in cfg.rules.iter().enumerate() {
            let resource = match r.resource.trim() {
                "*" | "" => None,
                res => Some(res.to_string()),
            };
            let role = match r.role.trim() {
                "*" | "" => None,
                role => Some(Role::parse(role).ok_or_else(|| {
                    format!("policy.rules[{i}] role 无效: {:?}（应为 admin/user/readonly/*）", r.role)
                })?),
            };
            let action = match r.action.trim() {
                "*" | "" => None,
                action => Some(Action::parse(action).ok_or_else(|| {
                    format!("policy.rules[{i}] action 无效: {:?}（应为 connect/list/*）", r.action)
                })?),
            };
            let allow = match r.decision.trim().to_ascii_lowercase().as_str() {
                "allow" => true,
                "deny" => false,
                other => {
                    return Err(format!(
                        "policy.rules[{i}] decision 无效: {other:?}（应为 allow/deny）"
                    ))
                }
            };
            rules.push(CompiledRule {
                resource,
                role,
                action,
                allow,
            });
        }
        Ok(Self { rules })
    }

    /// 判定：有序首条匹配生效；无匹配走默认矩阵
    /// （deny by default；admin 全通）。
    pub fn check(&self, resource: &str, role: Role, action: Action) -> Decision {
        let matched = self.rules.iter().find(|r| {
            r.resource.as_ref().map_or(true, |res| res == resource)
                && r.role.map_or(true, |ro| ro == role)
                && r.action.map_or(true, |ac| ac == action)
        });
        match matched {
            Some(r) => {
                if r.allow {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
            None => match role {
                Role::Admin => Decision::Allow,
                _ => Decision::Deny,
            },
        }
    }
}

impl Default for PolicyMatrix {
    fn default() -> Self {
        // 空矩阵 = 默认矩阵：deny by default、admin 全通。
        Self { rules: Vec::new() }
    }
}

/// 可热替换的策略矩阵句柄（读多写少：RwLock）。
#[derive(Default)]
pub struct PolicyHandle {
    matrix: RwLock<PolicyMatrix>,
}

impl PolicyHandle {
    pub fn from_matrix(matrix: PolicyMatrix) -> Self {
        Self {
            matrix: RwLock::new(matrix),
        }
    }

    /// 判定当前矩阵。
    pub fn check(&self, resource: &str, role: Role, action: Action) -> Decision {
        self.matrix
            .read()
            .expect("policy mutex poisoned")
            .check(resource, role, action)
    }

    /// 热加载：读取 config.toml 并编译 [policy] 段。
    /// 成功：原子换入新矩阵并返回新矩阵；失败：保留旧矩阵，返回 Err
    /// （含解析错误摘要，供调用方审计 warn）。
    pub fn reload_from_file(&self, path: impl AsRef<Path>) -> Result<PolicyMatrix, String> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| format!("读取配置失败: {e}"))?;
        let cfg = crate::config::GatewayConfig::parse(&text)?;
        let matrix = PolicyMatrix::from_config(&cfg.policy)?;
        *self.matrix.write().expect("policy mutex poisoned") = matrix.clone();
        Ok(matrix)
    }
}

// ---------------------------------------------------------------------
// 单元测试：admin/user/readonly × connect/list 全覆盖 + 默认矩阵 + 热加载
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn rule(res: &str, role: &str, action: &str, decision: &str) -> PolicyRuleConfig {
        PolicyRuleConfig {
            resource: res.into(),
            role: role.into(),
            action: action.into(),
            decision: decision.into(),
        }
    }

    fn matrix(rules: &[PolicyRuleConfig]) -> PolicyMatrix {
        PolicyMatrix::from_config(&PolicyConfig {
            rules: rules.to_vec(),
        })
        .expect("compile matrix")
    }

    const ALL_ROLES: [Role; 3] = [Role::Admin, Role::User, Role::ReadOnly];
    const ALL_ACTIONS: [Action; 2] = [Action::Connect, Action::List];

    #[test]
    fn default_matrix_admin_all_pass_others_deny() {
        let m = PolicyMatrix::default();
        for role in ALL_ROLES {
            for action in ALL_ACTIONS {
                let d = m.check("rdp-win01", role, action);
                if role == Role::Admin {
                    assert_eq!(d, Decision::Allow, "admin 默认全通 {action:?}");
                } else {
                    assert_eq!(d, Decision::Deny, "{role:?} 默认拒绝 {action:?}");
                }
            }
        }
    }

    /// 全矩阵单测：默认矩阵 + 显式规则下 admin/user/readonly ×
    /// connect/list 的每一格（允许/拒绝两种结论都覆盖）。
    #[test]
    fn matrix_full_coverage_roles_x_actions() {
        // 显式规则：user 允许 connect（所有资源）；readonly 允许 list；
        // readonly 对 rdp-win01 显式拒绝 connect（deny 优先级测试用）。
        let m = matrix(&[
            rule("*", "user", "connect", "allow"),
            rule("*", "readonly", "list", "allow"),
            rule("rdp-win01", "readonly", "connect", "deny"),
        ]);

        // user: connect 全允许（规则），list 走默认 -> deny
        for res in ["rdp-win01", "rdp-linux01"] {
            assert_eq!(m.check(res, Role::User, Action::Connect), Decision::Allow);
            assert_eq!(m.check(res, Role::User, Action::List), Decision::Deny);
        }
        // readonly: list 全允许（规则）；connect 对 rdp-win01 显式 deny，
        // 对其余资源无匹配 -> 默认 deny（只读角色无 connect 资格）
        assert_eq!(
            m.check("rdp-win01", Role::ReadOnly, Action::List),
            Decision::Allow
        );
        assert_eq!(
            m.check("rdp-win01", Role::ReadOnly, Action::Connect),
            Decision::Deny
        );
        assert_eq!(
            m.check("rdp-linux01", Role::ReadOnly, Action::Connect),
            Decision::Deny
        );
        // admin: 默认矩阵全通（规则未命中 admin 时）
        for res in ["rdp-win01", "rdp-linux01"] {
            for action in ALL_ACTIONS {
                assert_eq!(m.check(res, Role::Admin, action), Decision::Allow);
            }
        }
        // 允许路径的反面：显式拒绝 user connect 后，同格翻转为 Deny
        let m2 = matrix(&[rule("rdp-win01", "user", "connect", "deny")]);
        assert_eq!(
            m2.check("rdp-win01", Role::User, Action::Connect),
            Decision::Deny
        );
        // 而 admin 不受该规则影响（role 不匹配 -> 默认全通）
        assert_eq!(
            m2.check("rdp-win01", Role::Admin, Action::Connect),
            Decision::Allow
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        // 有序规则：先 allow 后 deny -> allow 生效（首条匹配优先）。
        let m = matrix(&[
            rule("rdp-win01", "user", "connect", "allow"),
            rule("*", "user", "connect", "deny"),
        ]);
        assert_eq!(
            m.check("rdp-win01", Role::User, Action::Connect),
            Decision::Allow
        );
        assert_eq!(
            m.check("other", Role::User, Action::Connect),
            Decision::Deny
        );
    }

    #[test]
    fn invalid_rule_fails_compilation() {
        // 坏规则整体失败（热加载安全回退的前提）
        assert!(PolicyMatrix::from_config(&PolicyConfig {
            rules: vec![rule("*", "superadmin", "connect", "allow")],
        })
        .is_err());
        assert!(PolicyMatrix::from_config(&PolicyConfig {
            rules: vec![rule("*", "user", "delete", "allow")],
        })
        .is_err());
        assert!(PolicyMatrix::from_config(&PolicyConfig {
            rules: vec![rule("*", "user", "connect", "maybe")],
        })
        .is_err());
    }

    #[test]
    fn canonical_role_mapping() {
        let mk = |r: &[&str]| r.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(canonical_role(&mk(&["ops", "admin"])), Role::Admin);
        assert_eq!(canonical_role(&mk(&["user"])), Role::User);
        assert_eq!(canonical_role(&mk(&["readonly"])), Role::ReadOnly);
        assert_eq!(canonical_role(&mk(&["guest", "unknown"])), Role::User);
        assert_eq!(canonical_role(&[]), Role::User);
    }

    // ---------------- 热加载（mtime 轮询的核心：reload_from_file） ----------------

    #[test]
    fn hot_reload_success_swaps_matrix() {
        let dir = std::env::temp_dir().join(format!("easyrdp-policy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("config.toml");

        let handle = PolicyHandle::default();
        // 初始：默认矩阵，user connect 拒绝
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Deny
        );

        std::fs::write(
            &path,
            "[[policy.rules]]\nresource = \"*\"\nrole = \"user\"\naction = \"connect\"\ndecision = \"allow\"\n",
        )
        .expect("write config");
        let m = handle.reload_from_file(&path).expect("reload");
        assert_eq!(m.rules.len(), 1);
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Allow
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hot_reload_failure_keeps_old_matrix() {
        let dir = std::env::temp_dir().join(format!("easyrdp-policy-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("config.toml");

        let handle = PolicyHandle::default();
        std::fs::write(
            &path,
            "[[policy.rules]]\nresource = \"*\"\nrole = \"user\"\naction = \"connect\"\ndecision = \"allow\"\n",
        )
        .expect("write good config");
        handle.reload_from_file(&path).expect("good reload");
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Allow
        );

        // 坏配置：decision 非法 -> 解析失败，旧矩阵保留
        std::fs::write(
            &path,
            "[[policy.rules]]\nresource = \"*\"\nrole = \"user\"\naction = \"connect\"\ndecision = \"perhaps\"\n",
        )
        .expect("write bad config");
        let err = handle.reload_from_file(&path).expect_err("bad reload");
        assert!(err.contains("decision"), "错误应指明 decision: {err}");
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Allow,
            "失败回退：旧策略必须保留"
        );

        // 坏 TOML 同样回退
        std::fs::write(&path, "not [valid toml").expect("write broken toml");
        assert!(handle.reload_from_file(&path).is_err());
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Allow
        );

        // 文件消失同样回退
        std::fs::remove_file(&path).expect("remove");
        assert!(handle.reload_from_file(&path).is_err());
        assert_eq!(
            handle.check("r1", Role::User, Action::Connect),
            Decision::Allow
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
