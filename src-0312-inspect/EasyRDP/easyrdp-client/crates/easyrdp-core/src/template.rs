//! 团队模板库（F2-V2 R-20261118-014）。
//!
//! `TemplateProfile` = SessionProfile 的可共享子集 + 模板元数据：
//! - 凭据剥离（发布门禁语义）：gateway token/password/username/otp 一律置空，
//!   主会话 username 保留可选；导出前强校验——profile 中含任何 `dpapi:` /
//!   `plain:` 密文前缀串或非空密码/token 字段 → 拒绝导出（防凭据泄漏）。
//! - 模板包格式（JSON）：
//!   单模板 `.erdp-template`：
//!   `{ "schema": "easyrdp-template-v1", "template": {..}, "sha256": "<hex>" }`
//!   多模板 `.erdp-templates`：
//!   `{ "schema": "easyrdp-template-v1", "templates": [..], "sha256": "<hex>" }`
//!   `sha256` = 去掉 sha256 字段后的包体 JSON 规范化字符串（serde_json
//!   BTreeMap 键序稳定）的 SHA-256 十六进制摘要；导入时先验完整性再验
//!   schema 与逐条模板合法性（复用 [`crate::schema`] 校验器）。

use crate::profile::SessionProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// 模板包 schema 版本。
pub const TEMPLATE_SCHEMA_VERSION: &str = "easyrdp-template-v1";

/// 模板元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateMeta {
    /// 模板 id（tpl-<纳秒时间戳>）；导入去重键。
    pub id: String,
    /// 模板名（展示用，可与来源连接名不同）。
    pub name: String,
    /// 模板描述（可选）。
    pub description: String,
    /// 分类标签（自由文本，UI 逗号分隔输入）。
    pub tags: Vec<String>,
    /// 创建时间（UTC，`YYYY-MM-DD HH:MM:SS`；空 = 未知，旧包兼容）。
    pub created_at: String,
    /// 模板版本（从 1 起；结构变更时递增）。
    pub version: u32,
}

impl Default for TemplateMeta {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            tags: Vec::new(),
            created_at: String::new(),
            version: 1,
        }
    }
}

/// 一条可共享的连接模板 = 元数据 + 剥离凭据后的 SessionProfile。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateProfile {
    pub meta: TemplateMeta,
    pub profile: SessionProfile,
}

impl Default for TemplateProfile {
    fn default() -> Self {
        Self {
            meta: TemplateMeta::default(),
            profile: SessionProfile::default(),
        }
    }
}

/// 当前 UTC 时间戳（`YYYY-MM-DD HH:MM:SS`，无 chrono 依赖的民用时换算）。
pub fn now_utc_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // Howard Hinnant civil_from_days（支持 1970 前后负数年份）
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 新模板 id（tpl-<纳秒时间戳>）。
pub fn new_template_id() -> String {
    format!(
        "tpl-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

impl TemplateProfile {
    /// 从一条连接构造模板：剥离网关凭据（token/password/username/otp），
    /// 主会话 username 保留可选；模板名默认取连接名（空则主机名）。
    pub fn from_session(p: &SessionProfile, name: &str, description: &str, tags: Vec<String>) -> Self {
        let mut profile = p.clone();
        if let Some(gw) = profile.gateway.as_mut() {
            gw.token = None;
            gw.password = None;
            gw.username = None;
            gw.otp_code = None;
        }
        let tpl_name = if name.trim().is_empty() {
            if p.name.trim().is_empty() {
                p.host.clone()
            } else {
                p.name.clone()
            }
        } else {
            name.trim().to_string()
        };
        Self {
            meta: TemplateMeta {
                id: new_template_id(),
                name: tpl_name,
                description: description.trim().to_string(),
                tags: tags
                    .into_iter()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                created_at: now_utc_string(),
                version: 1,
            },
            profile,
        }
    }

    /// 模板内嵌 profile 是否无凭据（模板库条目安全自检）。
    pub fn is_credential_free(&self) -> bool {
        find_credential_issues(&self.profile).is_empty()
    }
}

/// 导出前凭据泄漏扫描（发布门禁）。返回问题列表；非空 = 拒绝导出。
///
/// 规则：
/// 1. 任何非空 `password` / 网关 `token` / `password` / `otp_code` 字段；
/// 2. 任何以 `dpapi:`（Windows DPAPI 密文）或 `plain:`（非 Windows 回退）
///    开头的字符串——覆盖被加密的网关用户名等藏匿密文场景。
pub fn find_credential_issues(p: &SessionProfile) -> Vec<String> {
    let v = match serde_json::to_value(p) {
        Ok(v) => v,
        Err(e) => return vec![format!("profile 序列化失败: {e}")],
    };
    let mut issues = Vec::new();
    fn nonempty(v: Option<&Value>) -> bool {
        v.and_then(Value::as_str).map(|s| !s.trim().is_empty()).unwrap_or(false)
    }
    if nonempty(v.get("password")) {
        issues.push("含非空 password 字段".into());
    }
    if let Some(gw) = v.get("gateway") {
        for key in ["token", "password", "otp_code"] {
            if nonempty(gw.get(key)) {
                issues.push(format!("网关含非空 {key} 字段"));
            }
        }
    }
    // 递归扫描密文前缀（防藏匿：任何字段位置的 dpapi:/plain: 串都算凭据）
    fn walk(v: &Value, path: &str, issues: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                let t = s.trim();
                if t.starts_with("dpapi:") || t.starts_with("plain:") {
                    issues.push(format!("{path} 含密文格式串（dpapi:/plain: 前缀）"));
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{path}/{i}"), issues);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    walk(x, &format!("{path}/{k}"), issues);
                }
            }
            _ => {}
        }
    }
    walk(&v, "#", &mut issues);
    issues
}

// ---------------------------------------------------------------------------
// 模板包导出（含 sha256 完整性）
// ---------------------------------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 组包：schema + 条目 + sha256（sha256 = 去掉该字段后的包体序列化摘要；
/// serde_json Map 为 BTreeMap 键序，规范化稳定）。
fn build_package(v: &mut Value) -> String {
    {
        let obj = v.as_object_mut().expect("包体应为 object");
        obj.insert("schema".into(), Value::String(TEMPLATE_SCHEMA_VERSION.into()));
    }
    let body = serde_json::to_string(v).expect("包体序列化不会失败");
    let digest = sha256_hex(body.as_bytes());
    v.as_object_mut()
        .expect("包体应为 object")
        .insert("sha256".into(), Value::String(digest));
    serde_json::to_string_pretty(v).expect("包体序列化不会失败")
}

/// 包完整性校验：存在 sha256 字段且与去字段后的包体摘要一致。
fn verify_package_integrity(v: &Value) -> Result<(), String> {
    let claimed = v
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "模板包缺少 sha256 完整性字段".to_string())?;
    let mut body = v.clone();
    body.as_object_mut()
        .expect("包应为 object")
        .remove("sha256");
    let actual = sha256_hex(serde_json::to_string(&body).expect("包体序列化不会失败").as_bytes());
    if actual != claimed.trim().to_ascii_lowercase() {
        return Err(format!(
            "sha256 校验失败：包体已被修改（声明 {claimed}，实际 {actual}）"
        ));
    }
    Ok(())
}

/// 导出前强校验：模板必须无凭据且内嵌 profile 合法（host 非空等）。
pub fn assert_exportable(t: &TemplateProfile) -> Result<(), String> {
    let issues = find_credential_issues(&t.profile);
    if !issues.is_empty() {
        return Err(format!(
            "模板「{}」含凭据，已拒绝导出（防凭据泄漏）：{}",
            t.meta.name,
            issues.join("；")
        ));
    }
    if let Err(errs) = t.profile.validate() {
        return Err(format!(
            "模板「{}」内嵌连接配置不合法：{}",
            t.meta.name,
            errs.join("；")
        ));
    }
    Ok(())
}

/// 导出单模板 `.erdp-template` 文本。
pub fn export_single(t: &TemplateProfile) -> Result<String, String> {
    assert_exportable(t)?;
    Ok(build_package(&mut serde_json::json!({ "template": t })))
}

/// 导出多模板 `.erdp-templates` 文本（数组包；空列表报错）。
pub fn export_multi(ts: &[TemplateProfile]) -> Result<String, String> {
    if ts.is_empty() {
        return Err("没有可导出的模板".into());
    }
    for t in ts {
        assert_exportable(t)?;
    }
    Ok(build_package(&mut serde_json::json!({ "templates": ts })))
}

/// 解析结果：合法模板 + 逐条被拒原因（完整性/schema 包级失败则整体 Err）。
#[derive(Debug, Default, Clone)]
pub struct ParsedTemplatePackage {
    pub templates: Vec<TemplateProfile>,
    pub rejected: Vec<String>,
}

/// 导入模板包文本：验 sha256 完整性 → 验 schema 版本 → 逐条模板校验
/// （凭据扫描 + [`crate::schema`] 宽松校验 + 模型 validate）。
pub fn parse_package(text: &str) -> Result<ParsedTemplatePackage, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("模板包 JSON 非法: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| "模板包顶层应为 JSON 对象".to_string())?;
    let schema = obj
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| "模板包缺少 schema 版本字段".to_string())?;
    if schema != TEMPLATE_SCHEMA_VERSION {
        return Err(format!(
            "模板包 schema 版本不支持：{schema}（本机支持 {TEMPLATE_SCHEMA_VERSION}）"
        ));
    }
    verify_package_integrity(&v)?;

    let items: Vec<&Value> = if obj.contains_key("template") {
        vec![obj.get("template").expect("template 存在")]
    } else if let Some(arr) = obj.get("templates").and_then(Value::as_array) {
        arr.iter().collect()
    } else {
        return Err("模板包缺少 template/templates 字段".into());
    };

    let mut out = ParsedTemplatePackage::default();
    for (i, item) in items.iter().enumerate() {
        let label = item
            .get("meta")
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("<未命名>");
        let mut reject = |reason: String| {
            out.rejected
                .push(format!("第 {} 条（{label}）: {reason}", i + 1))
        };
        // 单条模板级附加完整性字段（可选：未来逐条签名/摘要预留）不参与
        // 反序列化校验；包级 sha256 已在上面整体验证。
        let t: TemplateProfile = match serde_json::from_value((*item).clone()) {
            Ok(t) => t,
            Err(e) => {
                reject(format!("反序列化失败: {e}"));
                continue;
            }
        };
        if !t.is_credential_free() {
            reject(find_credential_issues(&t.profile).join("；"));
            continue;
        }
        let pv = serde_json::to_value(&t.profile).expect("profile 序列化不会失败");
        if let Err(issues) = crate::schema::validate_profile_json(&pv, crate::schema::ValidationMode::Lenient) {
            reject(
                issues
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join("；"),
            );
            continue;
        }
        if let Err(errs) = t.profile.validate() {
            reject(errs.join("；"));
            continue;
        }
        out.templates.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{GatewayAuth, GatewayConfig};

    fn simple_profile(host: &str, port: u16, name: &str) -> SessionProfile {
        SessionProfile {
            name: name.into(),
            host: host.into(),
            port,
            ..SessionProfile::default()
        }
    }

    fn tpl(host: &str, port: u16, name: &str) -> TemplateProfile {
        TemplateProfile::from_session(&simple_profile(host, port, name), name, "描述", vec!["ops".into()])
    }

    // -----------------------------------------------------------------
    // 模板构造与凭据剥离
    // -----------------------------------------------------------------

    #[test]
    fn from_session_strips_gateway_credentials_keeps_username() {
        let mut p = simple_profile("10.0.0.5", 3389, "办公机");
        p.username = "alice".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw.example.com".into(),
            auth: GatewayAuth::UserPassword,
            resource_id: "rdp-win01".into(),
            token: Some("jwt-plain".into()),
            username: Some("gw-user".into()),
            password: Some("gw-pw".into()),
            otp_code: Some("123456".into()),
            ..GatewayConfig::default()
        });
        let t = TemplateProfile::from_session(&p, "", "desc", vec!["a".into(), " b ".into(), "".into()]);
        // 网关凭据全部剥离；连接 username 保留
        let gw = t.profile.gateway.as_ref().unwrap();
        assert_eq!(gw.token, None);
        assert_eq!(gw.password, None);
        assert_eq!(gw.username, None, "网关 username 属凭据，必须剥离");
        assert_eq!(gw.otp_code, None);
        assert_eq!(gw.hostname, "gw.example.com", "网关非凭据字段保留");
        assert_eq!(t.profile.username, "alice", "主 username 可选保留");
        assert!(t.is_credential_free());
        // 元数据：名字回退连接名、标签去空白去空
        assert_eq!(t.meta.name, "办公机");
        assert_eq!(t.meta.tags, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(t.meta.version, 1);
        assert!(t.meta.id.starts_with("tpl-"));
        assert!(t.meta.created_at.contains('-'), "created_at 应为民用时间格式");
    }

    // -----------------------------------------------------------------
    // 导出门禁：含凭据（含 dpapi: 前缀密文）→ 拒绝
    // -----------------------------------------------------------------

    /// 构造一个「未剥离凭据」的模板（模拟旧数据/上层误传，验证导出门禁拦截）。
    fn dirty_tpl(host: &str, name: &str, gw: GatewayConfig) -> TemplateProfile {
        let mut p = simple_profile(host, 3389, name);
        p.gateway = Some(gw);
        TemplateProfile {
            meta: TemplateMeta {
                id: format!("tpl-{host}"),
                name: name.into(),
                ..TemplateMeta::default()
            },
            profile: p,
        }
    }

    #[test]
    fn export_rejects_nonempty_password_field() {
        let t = dirty_tpl(
            "10.0.0.5",
            "x",
            GatewayConfig {
                hostname: "gw".into(),
                auth: GatewayAuth::Token,
                password: Some("pw".into()),
                ..GatewayConfig::default()
            },
        );
        let err = export_single(&t).unwrap_err();
        assert!(err.contains("拒绝导出"), "{err}");
        assert!(err.contains("password"), "{err}");
    }

    #[test]
    fn export_rejects_dpapi_prefixed_ciphertext_anywhere() {
        let t = dirty_tpl(
            "10.0.0.5",
            "x",
            GatewayConfig {
                hostname: "gw".into(),
                auth: GatewayAuth::Token,
                // 模拟 DPAPI 密文（如旧数据未剥离的加密网关用户名）
                username: Some("dpapi:QUJDREVGR0hJSktMTU5PUA==".into()),
                ..GatewayConfig::default()
            },
        );
        assert!(!t.is_credential_free());
        let err = export_single(&t).unwrap_err();
        assert!(err.contains("dpapi:"), "{err}");

        // plain: 前缀（非 Windows 回退密文）同样拒绝
        let t2 = dirty_tpl(
            "10.0.0.6",
            "y",
            GatewayConfig {
                hostname: "gw".into(),
                auth: GatewayAuth::Inherit,
                token: Some("plain:old-token".into()),
                ..GatewayConfig::default()
            },
        );
        assert!(export_single(&t2).unwrap_err().contains("plain:"));
    }

    #[test]
    fn export_rejects_gateway_token_and_otp_fields() {
        let mut p = simple_profile("10.0.0.5", 3389, "x");
        p.gateway = Some(GatewayConfig {
            hostname: "gw".into(),
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            otp_code: Some("654321".into()),
            ..GatewayConfig::default()
        });
        let t = TemplateProfile::from_session(&p, "", "", vec![]);
        // from_session 已剥离 → 可导出；但手工构造的含凭据模板必须被门禁拦截
        let mut bad = t.clone();
        bad.profile.gateway.as_mut().unwrap().token = Some("jwt".into());
        let err = export_single(&bad).unwrap_err();
        assert!(err.contains("token"), "{err}");
        let mut bad2 = t.clone();
        bad2.profile.gateway.as_mut().unwrap().otp_code = Some("654321".into());
        assert!(export_single(&bad2).unwrap_err().contains("otp_code"));
    }

    #[test]
    fn export_multi_rejects_if_any_template_has_credentials() {
        let good = tpl("10.0.0.5", 3389, "good");
        let mut bad = tpl("10.0.0.6", 3389, "bad");
        bad.profile.gateway = Some(GatewayConfig {
            hostname: "gw".into(),
            auth: GatewayAuth::Token,
            token: Some("jwt".into()),
            ..GatewayConfig::default()
        });
        assert!(export_multi(&[good.clone(), bad]).is_err());
        // 全部干净 → 成功
        let good2 = tpl("10.0.0.6", 3390, "good2");
        assert!(export_multi(&[good, good2]).is_ok());
        // 空列表报错
        assert!(export_multi(&[]).is_err());
    }

    // -----------------------------------------------------------------
    // sha256 完整性与 schema 校验
    // -----------------------------------------------------------------

    #[test]
    fn exported_package_contains_schema_and_sha256() {
        let text = export_single(&tpl("h1", 3389, "t1")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema"], TEMPLATE_SCHEMA_VERSION);
        let sha = v["sha256"].as_str().unwrap();
        assert_eq!(sha.len(), 64);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tampered_package_fails_sha256_check() {
        let text = export_single(&tpl("h1", 3389, "t1")).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        // 篡改主机端口
        v["template"]["profile"]["port"] = serde_json::json!(3390);
        let tampered = serde_json::to_string_pretty(&v).unwrap();
        let err = parse_package(&tampered).unwrap_err();
        assert!(err.contains("sha256"), "{err}");

        // 删除 sha256 字段
        let mut v2: Value = serde_json::from_str(&text).unwrap();
        v2.as_object_mut().unwrap().remove("sha256");
        assert!(parse_package(&serde_json::to_string(&v2).unwrap())
            .unwrap_err()
            .contains("sha256"));
    }

    #[test]
    fn wrong_schema_version_rejected() {
        let text = export_single(&tpl("h1", 3389, "t1")).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["schema"] = serde_json::json!("easyrdp-template-v999");
        let err = parse_package(&serde_json::to_string_pretty(&v).unwrap()).unwrap_err();
        assert!(err.contains("schema"), "{err}");
        // 缺 schema 字段
        let mut v2 = v.clone();
        v2.as_object_mut().unwrap().remove("schema");
        v2.as_object_mut().unwrap().remove("sha256");
        assert!(parse_package(&serde_json::to_string(&v2).unwrap()).is_err());
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(parse_package("{ not json").unwrap_err().contains("JSON 非法"));
        assert!(parse_package("[1,2]").unwrap_err().contains("对象"));
    }

    // -----------------------------------------------------------------
    // 导入回环与去重
    // -----------------------------------------------------------------

    #[test]
    fn single_export_import_roundtrip() {
        let t = tpl("rdp.corp.example.com", 3390, "团队模板A");
        let text = export_single(&t).unwrap();
        let parsed = parse_package(&text).unwrap();
        assert!(parsed.rejected.is_empty());
        assert_eq!(parsed.templates.len(), 1);
        let back = &parsed.templates[0];
        assert_eq!(back, &t, "单模板包应无损往返");
        assert_eq!(back.meta.tags, vec!["ops".to_string()]);
        assert_eq!(back.profile.port, 3390);
    }

    #[test]
    fn multi_export_import_roundtrip_preserves_order() {
        let ts = vec![tpl("h1", 3389, "a"), tpl("h2", 3390, "b"), tpl("h3", 3391, "c")];
        let text = export_multi(&ts).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["templates"].as_array().unwrap().len(),
            3,
            "多模板包为数组结构"
        );
        let parsed = parse_package(&text).unwrap();
        assert_eq!(parsed.templates, ts, "数组包按序无损往返");
    }

    /// 导入去重：库内按 meta.id 判重（本文件定义的通用去重辅助，UI/存储层复用）。
    #[test]
    fn import_dedup_by_template_id() {
        let existing = vec![tpl("h1", 3389, "a"), tpl("h2", 3390, "b")];
        let incoming = vec![
            tpl("h9", 3389, "new"),
            existing[0].clone(), // 同 id → 重复
            tpl("h8", 3390, "c"),
        ];
        let (add, dup) = partition_duplicates(&existing, &incoming);
        assert_eq!(add.len(), 2);
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].meta.id, existing[0].meta.id);
    }

    // -----------------------------------------------------------------
    // serde 兼容
    // -----------------------------------------------------------------

    /// 旧/部分字段 JSON：缺 meta 字段 serde default 补齐，roundtrip 不丢数据。
    #[test]
    fn serde_backward_compat_partial_meta() {
        let old = r#"{
            "meta": { "id": "tpl-1", "name": "旧模板" },
            "profile": { "host": "10.0.0.5", "port": 3389 }
        }"#;
        let t: TemplateProfile = serde_json::from_str(old).expect("旧 JSON（缺 description/tags 等）应可解析");
        assert_eq!(t.meta.name, "旧模板");
        assert_eq!(t.meta.version, 1, "缺省模板版本 = 1");
        assert!(t.meta.tags.is_empty());
        assert!(t.meta.description.is_empty());
        assert_eq!(t.profile.port, 3389);
        assert_eq!(t.profile.gateway, None);
        // roundtrip
        let back: TemplateProfile =
            serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(t, back);
    }

    /// 顶层缺 meta / profile 的 JSON → default 补空（不 panic，校验阶段拦截）。
    #[test]
    fn serde_backward_compat_missing_top_fields() {
        let t: TemplateProfile = serde_json::from_str("{}").unwrap();
        assert_eq!(t, TemplateProfile::default());
    }

    /// 导出的包文本经 serde Value 中转（模拟前端查看/再保存）后仍可导入。
    #[test]
    fn package_survives_value_reserialize() {
        let t = tpl("h1", 3389, "rt");
        let text = export_single(&t).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let re = serde_json::to_string_pretty(&v).unwrap();
        let parsed = parse_package(&re).unwrap();
        assert_eq!(parsed.templates.len(), 1);
        assert_eq!(parsed.templates[0], t);
    }
}

/// 通用导入去重辅助：按 meta.id 把 incoming 分为（新增, 重复）。
/// 库内已有同 id 模板 → 计入重复（上层按 Skip 策略处理）。
pub fn partition_duplicates(
    existing: &[TemplateProfile],
    incoming: &[TemplateProfile],
) -> (Vec<TemplateProfile>, Vec<TemplateProfile>) {
    let mut seen: std::collections::HashSet<String> =
        existing.iter().map(|t| t.meta.id.clone()).collect();
    let mut add = Vec::new();
    let mut dup = Vec::new();
    for t in incoming {
        if seen.insert(t.meta.id.clone()) {
            add.push(t.clone());
        } else {
            dup.push(t.clone());
        }
    }
    (add, dup)
}
