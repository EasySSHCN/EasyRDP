//! EasyRDP 核心库：会话配置模型与 .rdp 导入/导出。

pub mod export;
pub mod profile;
pub mod rdp_import;

pub use profile::{AuthMethod, GatewayAuth, GatewayConfig, Resolution, SessionProfile};
