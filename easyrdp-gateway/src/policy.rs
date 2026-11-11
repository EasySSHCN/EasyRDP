//! 策略模块：重定向策略检查。
//!
//! check_redirect(role, redirect_kind) -> Allow/Deny
//! 默认矩阵：运维（ops）= 全部重定向允许；外包（contractor）= 全部禁止。
//! 其余角色默认禁用未知重定向类型（最小权限）。

use serde::{Deserialize, Serialize};

/// 重定向类型（对齐 RDP 驱动器/打印机/剪贴板等资源重定向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedirectKind {
    /// 驱动器重定向
    Drive,
    /// 打印机重定向
    Printer,
    /// 剪贴板
    Clipboard,
    /// 音频
    Audio,
    /// USB 设备
    Usb,
}

/// 策略结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

/// 角色（内置两个默认角色；其余自定义角色走自定义矩阵）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// 运维：全开
    Ops,
    /// 外包：全禁
    Contractor,
    /// 自定义/未知角色
    Other(&'static str),
}

/// 默认重定向矩阵。
const ALL_KINDS: [RedirectKind; 5] = [
    RedirectKind::Drive,
    RedirectKind::Printer,
    RedirectKind::Clipboard,
    RedirectKind::Audio,
    RedirectKind::Usb,
];

/// 检查某角色是否允许某类重定向。
///
/// 默认策略：
/// - ops（运维）       -> Allow（全开）
/// - contractor（外包）-> Deny（全禁）
/// - 其他角色          -> Deny（最小权限，显式配置才放行）
pub fn check_redirect(role: Role, kind: RedirectKind) -> Decision {
    let _ = ALL_KINDS; // 保留常量，便于后续接入自定义矩阵
    match role {
        Role::Ops => Decision::Allow,
        Role::Contractor => Decision::Deny,
        Role::Other(_) => Decision::Deny,
        // kind 目前不影响结论；接入自定义矩阵后按 (role, kind) 查表。
        // 显式使用 kind 以避免未使用告警：
        // 每种重定向类型在默认矩阵中语义一致（见上）。
        // no-op:
    }
    .tap(|d| log_decision(d, role, kind))
}

/// 小工具：表达"链式副注"，避免 if/else 噪音（仅日志用途）。
trait Tap: Sized {
    fn tap<F: FnOnce(&Self)>(self, f: F) -> Self {
        f(&self);
        self
    }
}
impl<T> Tap for T {}

fn log_decision(d: &Decision, role: Role, kind: RedirectKind) {
    tracing::debug!(?d, ?role, ?kind, "redirect policy decision");
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_is_allowed_for_all_kinds() {
        for kind in ALL_KINDS {
            assert_eq!(check_redirect(Role::Ops, kind), Decision::Allow);
        }
    }

    #[test]
    fn contractor_is_denied_for_all_kinds() {
        for kind in ALL_KINDS {
            assert_eq!(check_redirect(Role::Contractor, kind), Decision::Deny);
        }
    }

    #[test]
    fn unknown_role_defaults_to_deny() {
        assert_eq!(
            check_redirect(Role::Other("guest"), RedirectKind::Clipboard),
            Decision::Deny
        );
    }
}
