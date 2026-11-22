//! EasyRDP 核心库：会话配置模型与 .rdp 导入/导出。
pub mod batch_import;
pub mod export;
/// libfko 2.6.10 最小 FFI 绑定（fwknop 官方 C 库 vendor 编译）。
pub mod fko;
pub mod git_sync;
/// 分组树模型（ServerGroup / 主机分组校验）。
pub mod groups;
/// 端口敲门（knockd 序列敲门 / fwknop SPA）客户端兼容。
pub mod knock;
pub mod profile;
pub mod rdp_import;
pub mod schema;
pub mod secret;
pub mod template;

pub use profile::{AuthMethod, GatewayAuth, GatewayConfig, Resolution, SessionProfile};
pub use schema::{
    profile_json_schema, validate_profile_json, JsonProfileIssue, Severity, ValidationMode,
};
pub use template::{
    export_multi, export_single, find_credential_issues, new_template_id, parse_package,
    partition_duplicates, ParsedTemplatePackage, TemplateMeta, TemplateProfile,
    TEMPLATE_SCHEMA_VERSION,
};
