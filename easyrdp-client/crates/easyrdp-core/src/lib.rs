//! EasyRDP 核心库：会话配置模型与 .rdp 导入/导出。

pub mod batch_import;
pub mod export;
pub mod groups;
pub mod profile;
pub mod rdp_import;
pub mod schema;
pub mod secret;

pub use profile::{AuthMethod, GatewayAuth, GatewayConfig, Resolution, SessionProfile};
pub use schema::{
    profile_json_schema, validate_profile_json, JsonProfileIssue, Severity, ValidationMode,
};
