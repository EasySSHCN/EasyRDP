//! JSON Profile Schema（draft-07）与导入前轻量校验。
//!
//! 设计要点：
//! - Schema 从 [`SessionProfile`] 模型手写派生，单测固化：schema 的
//!   properties 与模型 serde 序列化字段集合逐一断言（防漂移）。
//! - 校验器为手写轻量实现（不引外部 jsonschema 库，零新增依赖），
//!   在 serde 反序列化**之前**对原始 `serde_json::Value` 校验，
//!   这样类型错误（如 port 为字符串）、未知字段、端口越界都能在
//!   serde 静默补默认值之前被发现。
//! - 错误/警告均带 JSON Pointer 路径（RFC 6901，如 `#/host`、
//!   `#/gateway/port`）便于 UI 定位。
//! - 宽松模式（默认）：硬错误（缺失/类型/越界）导致该条导入失败，
//!   未知字段只产生警告不阻塞；strict 模式：任何问题（含未知字段
//!   警告）都视为失败（全量失败语义，由调用方逐条收集）。

use serde_json::Value;
use std::fmt;

/// Schema 使用的 JSON Schema 版本。
pub const SCHEMA_DRAFT: &str = "http://json-schema.org/draft-07/schema#";

/// 校验严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 硬错误：缺失必填 / 类型错误 / 端口越界 / 枚举值非法。
    Error,
    /// 警告：未知字段（宽松模式放行，strict 模式视为失败）。
    Warning,
}

/// 一条校验问题。`pointer` 为 JSON Pointer（RFC 6901）路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonProfileIssue {
    pub pointer: String,
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for JsonProfileIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pointer, self.message)
    }
}

/// 校验模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    /// 宽松：硬错误失败，未知字段仅警告。
    Lenient,
    /// 严格：任何问题（含未知字段）都失败。
    Strict,
}

/// SessionProfile 的 JSON Schema（draft-07）。
/// 手写派生自 `crates/easyrdp-core/src/profile.rs` 的模型，
/// 由 `schema_matches_model` 单测固化，字段增删必须同步本常量。
pub fn profile_json_schema() -> &'static str {
    PROFILE_SCHEMA
}

const PROFILE_SCHEMA: &str = r##"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "$id": "https://easyrdp.example/schemas/profile-v1.json",
  "title": "EasyRDP SessionProfile",
  "description": "EasyRDP JSON Profile（单条连接）。gateway 为 null 表示直连。",
  "type": "object",
  "additionalProperties": false,
  "required": ["host"],
  "properties": {
    "name": { "type": "string", "description": "配置显示名" },
    "host": { "type": "string", "minLength": 1, "description": "目标主机（IP 或域名，不含端口）" },
    "port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 3389, "description": "RDP 端口" },
    "username": { "type": "string" },
    "auth_method": {
      "type": "string",
      "enum": ["password", "sso", "smart_card"],
      "default": "password"
    },
    "resolution": {
      "description": "分辨率策略：fit_window / fullscreen，或 {\"custom\": {\"w\": .., \"h\": ..}}",
      "oneOf": [
        { "type": "string", "enum": ["fit_window", "fullscreen"] },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["custom"],
          "properties": {
            "custom": {
              "type": "object",
              "additionalProperties": false,
              "required": ["w", "h"],
              "properties": {
                "w": { "type": "integer", "minimum": 1 },
                "h": { "type": "integer", "minimum": 1 }
              }
            }
          }
        }
      ],
      "default": "fit_window"
    },
    "color_depth": { "type": "integer", "enum": [16, 24, 32], "default": 32, "description": "颜色深度（bit）" },
    "order": { "type": "integer", "default": 0, "description": "连接列表手动排序值（越小越靠前）" },
    "monitor": { "type": ["integer", "null"], "description": "指定显示器序号（null = 主显示器）" },
    "multi_monitor": { "type": "boolean", "default": false, "description": "多显示器扩展到全部屏幕" },
    "redirect": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "clipboard": { "type": "boolean", "default": true },
        "printers": { "type": "boolean", "default": false },
        "drives": { "type": "boolean", "default": false },
        "camera": { "type": "boolean", "default": false }
      }
    },
    "gateway": {
      "description": "RD Gateway 网关配置；null 表示直连",
      "oneOf": [
        { "type": "null" },
        { "$ref": "#/definitions/gatewayConfig" }
      ]
    }
  },
  "definitions": {
    "gatewayAuth": {
      "type": "object",
      "additionalProperties": false,
      "required": ["type"],
      "properties": {
        "type": {
          "type": "string",
          "enum": ["otp_cookie", "inherit", "token", "user_password"]
        }
      }
    },
    "gatewayConfig": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "hostname": { "type": "string" },
        "port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 443 },
        "use_tls": { "type": "boolean", "default": true },
        "auth": { "$ref": "#/definitions/gatewayAuth" },
        "resource_id": { "type": "string" },
        "token": { "type": ["string", "null"] },
        "username": { "type": ["string", "null"] },
        "password": { "type": ["string", "null"] },
        "allow_self_signed": { "type": "boolean", "default": false },
        "otp_code": { "type": ["string", "null"] }
      }
    }
  }
}
"##;

// ---------------------------------------------------------------------------
// 轻量校验器（手写，不依赖外部 jsonschema 库）
// ---------------------------------------------------------------------------

fn err(pointer: &str, message: impl Into<String>) -> JsonProfileIssue {
    JsonProfileIssue {
        pointer: pointer.to_string(),
        severity: Severity::Error,
        message: message.into(),
    }
}

fn warn(pointer: &str, message: impl Into<String>) -> JsonProfileIssue {
    JsonProfileIssue {
        pointer: pointer.to_string(),
        severity: Severity::Warning,
        message: message.into(),
    }
}

fn is_str(v: &Value) -> bool {
    matches!(v, Value::String(_))
}

fn is_uint(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_u64() || n.is_i64())
}

fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_i64().map(|i| i.max(0) as u64)),
        _ => None,
    }
}

fn is_bool(v: &Value) -> bool {
    matches!(v, Value::Bool(_))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 检查整数是否在 [lo, hi]，越界/非整数都产出硬错误。
fn check_port(obj: &serde_json::Map<String, Value>, key: &str, base: &str, issues: &mut Vec<JsonProfileIssue>) {
    if let Some(v) = obj.get(key) {
        let pointer = format!("{base}/{key}");
        match as_u64(v) {
            Some(p) if (1..=65535).contains(&p) => {}
            Some(p) => issues.push(err(&pointer, format!("端口越界: {p}（须为 1-65535）"))),
            None => issues.push(err(
                &pointer,
                format!("类型错误：应为 integer，实际 {}", type_name(v)),
            )),
        }
    }
}

/// 未知字段检查：遍历对象键，不在 known 中的记警告。
fn check_unknown(obj: &serde_json::Map<String, Value>, known: &[&str], base: &str, issues: &mut Vec<JsonProfileIssue>) {
    for k in obj.keys() {
        if !known.contains(&k.as_str()) {
            issues.push(warn(&format!("{base}/{k}"), format!("未知字段: {k}")));
        }
    }
}

const AUTH_METHODS: [&str; 3] = ["password", "sso", "smart_card"];
const GATEWAY_AUTH_TYPES: [&str; 4] = ["otp_cookie", "inherit", "token", "user_password"];

/// 校验一个 JSON Profile 值（serde 反序列化之前调用）。
///
/// - [`ValidationMode::Lenient`]：硬错误 → `Err(硬错误列表)`；仅未知字段
///   警告 → `Ok(警告列表)`。
/// - [`ValidationMode::Strict`]：任何问题（含未知字段警告）→
///   `Err(全部问题列表)`。
///
/// 返回 `Ok` 后才应进入既有 `SessionProfile::validate`（模型级校验）。
pub fn validate_profile_json(
    value: &Value,
    mode: ValidationMode,
) -> Result<Vec<JsonProfileIssue>, Vec<JsonProfileIssue>> {
    let mut issues = Vec::new();

    let obj = match value {
        Value::Object(o) => o,
        other => {
            issues.push(err("#", format!("类型错误：应为 object，实际 {}", type_name(other))));
            return finish(issues, mode);
        }
    };

    // ---- 未知字段（警告）----
    const TOP_LEVEL: [&str; 12] = [
        "name", "host", "port", "username", "auth_method", "resolution",
        "color_depth", "redirect", "gateway", "order", "monitor", "multi_monitor",
    ];
    check_unknown(obj, &TOP_LEVEL, "#", &mut issues);

    // ---- 必填 host ----
    match obj.get("host") {
        None => issues.push(err("#", "缺少必填字段: host")),
        Some(v) if !is_str(v) => issues.push(err(
            "#/host",
            format!("类型错误：应为 string，实际 {}", type_name(v)),
        )),
        Some(Value::String(s)) if s.trim().is_empty() => {
            issues.push(err("#/host", "host 不能为空"))
        }
        _ => {}
    }

    // ---- 类型检查 ----
    for key in ["name", "username"] {
        if let Some(v) = obj.get(key) {
            if !is_str(v) {
                issues.push(err(
                    &format!("#/{key}"),
                    format!("类型错误：应为 string，实际 {}", type_name(v)),
                ));
            }
        }
    }
    check_port(obj, "port", "#", &mut issues);

    if let Some(v) = obj.get("order") {
        if !is_uint(v) && !(matches!(v, Value::Number(n) if n.is_i64())) {
            issues.push(err(
                "#/order",
                format!("类型错误：应为 integer，实际 {}", type_name(v)),
            ));
        }
    }

    // F5-V1：monitor（integer 或 null）与 multi_monitor（boolean）
    if let Some(v) = obj.get("monitor") {
        if !v.is_null() && !is_uint(v) {
            issues.push(err(
                "#/monitor",
                format!("类型错误：应为 integer 或 null，实际 {}", type_name(v)),
            ));
        }
    }
    if let Some(v) = obj.get("multi_monitor") {
        if !is_bool(v) {
            issues.push(err(
                "#/multi_monitor",
                format!("类型错误：应为 boolean，实际 {}", type_name(v)),
            ));
        }
    }

    if let Some(v) = obj.get("color_depth") {
        match as_u64(v) {
            Some(d @ 16 | d @ 24 | d @ 32) => {
                let _ = d;
            }
            Some(d) => issues.push(err(
                "#/color_depth",
                format!("非法颜色深度: {d}（允许 16/24/32）"),
            )),
            None => issues.push(err(
                "#/color_depth",
                format!("类型错误：应为 integer，实际 {}", type_name(v)),
            )),
        }
    }

    if let Some(v) = obj.get("auth_method") {
        let ok = is_str(v)
            && AUTH_METHODS.contains(&v.as_str().unwrap_or_default());
        if !ok {
            issues.push(err(
                "#/auth_method",
                format!(
                    "非法枚举值（允许 {:?}），实际 {}",
                    AUTH_METHODS,
                    v.as_str().unwrap_or_else(|| type_name(v))
                ),
            ));
        }
    }

    // ---- resolution ----
    if let Some(v) = obj.get("resolution") {
        check_resolution(v, &mut issues);
    }

    // ---- redirect ----
    if let Some(v) = obj.get("redirect") {
        match v {
            Value::Object(ro) => {
                check_unknown(ro, &["clipboard", "printers", "drives", "camera"], "#/redirect", &mut issues);
                for key in ["clipboard", "printers", "drives", "camera"] {
                    if let Some(b) = ro.get(key) {
                        if !is_bool(b) {
                            issues.push(err(
                                &format!("#/redirect/{key}"),
                                format!("类型错误：应为 boolean，实际 {}", type_name(b)),
                            ));
                        }
                    }
                }
            }
            other => issues.push(err(
                "#/redirect",
                format!("类型错误：应为 object，实际 {}", type_name(other)),
            )),
        }
    }

    // ---- gateway ----
    if let Some(v) = obj.get("gateway") {
        check_gateway(v, &mut issues);
    }

    finish(issues, mode)
}

fn check_resolution(v: &Value, issues: &mut Vec<JsonProfileIssue>) {
    match v {
        Value::String(s) => {
            if !matches!(s.as_str(), "fit_window" | "fullscreen") {
                issues.push(err(
                    "#/resolution",
                    format!("非法枚举值（允许 fit_window/fullscreen 或 {{\"custom\":{{w,h}}}}），实际 {s}"),
                ));
            }
        }
        Value::Object(o) => match o.get("custom") {
            Some(Value::Object(co)) => {
                check_unknown(co, &["w", "h"], "#/resolution/custom", issues);
                for key in ["w", "h"] {
                    match co.get(key) {
                        Some(n) if is_uint(n) && as_u64(n).unwrap_or(0) >= 1 => {}
                        Some(n) if is_uint(n) => issues.push(err(
                            &format!("#/resolution/custom/{key}"),
                            format!("分辨率须 >= 1，实际 {}", as_u64(n).unwrap_or(0)),
                        )),
                        Some(n) => issues.push(err(
                            &format!("#/resolution/custom/{key}"),
                            format!("类型错误：应为 integer，实际 {}", type_name(n)),
                        )),
                        None => issues.push(err(
                            "#/resolution/custom",
                            format!("缺少必填字段: {key}"),
                        )),
                    }
                }
            }
            Some(other) => issues.push(err(
                "#/resolution/custom",
                format!("类型错误：应为 object，实际 {}", type_name(other)),
            )),
            None => issues.push(err("#/resolution", "对象形式 resolution 缺少必填字段: custom")),
        },
        _ => issues.push(err(
            "#/resolution",
            format!("类型错误：应为 string 或 object，实际 {}", type_name(v)),
        )),
    }
}

fn check_gateway(v: &Value, issues: &mut Vec<JsonProfileIssue>) {
    match v {
        Value::Null => {}
        Value::Object(g) => {
            const GW_FIELDS: [&str; 10] = [
                "hostname", "port", "use_tls", "auth", "resource_id", "token", "username", "password",
                "allow_self_signed", "otp_code",
            ];
            check_unknown(g, &GW_FIELDS, "#/gateway", issues);

            if let Some(h) = g.get("hostname") {
                if !is_str(h) {
                    issues.push(err(
                        "#/gateway/hostname",
                        format!("类型错误：应为 string，实际 {}", type_name(h)),
                    ));
                }
            }
            check_port(g, "port", "#/gateway", issues);

            if let Some(t) = g.get("use_tls") {
                if !is_bool(t) {
                    issues.push(err(
                        "#/gateway/use_tls",
                        format!("类型错误：应为 boolean，实际 {}", type_name(t)),
                    ));
                }
            }
            // F4-V1：allow_self_signed（boolean，默认 false）
            if let Some(t) = g.get("allow_self_signed") {
                if !is_bool(t) {
                    issues.push(err(
                        "#/gateway/allow_self_signed",
                        format!("类型错误：应为 boolean，实际 {}", type_name(t)),
                    ));
                }
            }
            // F4-V1：otp_code（string 或 null，运行时一次性字段）
            if let Some(x) = g.get("otp_code") {
                if !x.is_null() && !is_str(x) {
                    issues.push(err(
                        "#/gateway/otp_code",
                        format!("类型错误：应为 string 或 null，实际 {}", type_name(x)),
                    ));
                }
            }
            for key in ["resource_id"] {
                if let Some(r) = g.get(key) {
                    if !is_str(r) {
                        issues.push(err(
                            &format!("#/gateway/{key}"),
                            format!("类型错误：应为 string，实际 {}", type_name(r)),
                        ));
                    }
                }
            }
            for key in ["token", "username", "password"] {
                if let Some(x) = g.get(key) {
                    if !x.is_null() && !is_str(x) {
                        issues.push(err(
                            &format!("#/gateway/{key}"),
                            format!("类型错误：应为 string 或 null，实际 {}", type_name(x)),
                        ));
                    }
                }
            }
            if let Some(a) = g.get("auth") {
                match a {
                    Value::Object(ao) => {
                        check_unknown(ao, &["type"], "#/gateway/auth", issues);
                        match ao.get("type") {
                            None => issues.push(err("#/gateway/auth", "缺少必填字段: type")),
                            Some(t) if is_str(t)
                                && GATEWAY_AUTH_TYPES.contains(&t.as_str().unwrap_or_default()) => {}
                            Some(t) => issues.push(err(
                                "#/gateway/auth/type",
                                format!(
                                    "非法枚举值（允许 {:?}），实际 {}",
                                    GATEWAY_AUTH_TYPES,
                                    t.as_str().unwrap_or_else(|| type_name(t))
                                ),
                            )),
                        }
                    }
                    other => issues.push(err(
                        "#/gateway/auth",
                        format!("类型错误：应为 object，实际 {}", type_name(other)),
                    )),
                }
            }
        }
        other => issues.push(err(
            "#/gateway",
            format!("类型错误：应为 object 或 null，实际 {}", type_name(other)),
        )),
    }
}

/// 按 mode 汇总：Lenient 只看 Error；Strict 看全部。
fn finish(
    issues: Vec<JsonProfileIssue>,
    mode: ValidationMode,
) -> Result<Vec<JsonProfileIssue>, Vec<JsonProfileIssue>> {
    let hard: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).cloned().collect();
    if !hard.is_empty() {
        return Err(hard);
    }
    if mode == ValidationMode::Strict && !issues.is_empty() {
        return Err(issues);
    }
    Ok(issues.into_iter().filter(|i| i.severity == Severity::Warning).collect())
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SessionProfile;
    use serde_json::json;

    fn valid_profile() -> Value {
        json!({
            "name": "office",
            "host": "192.168.1.100",
            "port": 3389,
            "username": "admin",
            "auth_method": "password",
            "resolution": "fit_window",
            "color_depth": 32,
            "redirect": { "clipboard": true, "printers": false, "drives": false, "camera": false },
            "gateway": null
        })
    }

    // ---------------- schema 固化（防漂移） ----------------

    /// schema 必须是合法 JSON 且声明 draft-07。
    #[test]
    fn schema_is_valid_json_draft07() {
        let s: Value = serde_json::from_str(profile_json_schema()).expect("schema 应为合法 JSON");
        assert_eq!(s["$schema"], SCHEMA_DRAFT);
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!(["host"]));
    }

    /// schema properties 与模型 serde 序列化字段集合完全一致（防漂移核心断言）。
    #[test]
    fn schema_properties_match_model_fields() {
        let schema: Value = serde_json::from_str(profile_json_schema()).unwrap();
        let schema_keys: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();

        let model = SessionProfile::default();
        let model_keys: Vec<String> = serde_json::to_value(&model)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();

        assert_eq!(schema_keys.len(), model_keys.len(), "字段数量漂移");
        for k in &model_keys {
            assert!(
                schema_keys.contains(&k),
                "模型字段 {k} 在 schema.properties 中缺失"
            );
        }
        for k in schema_keys {
            assert!(
                model_keys.contains(k),
                "schema 多出模型不存在的字段: {k}"
            );
        }
    }

    /// schema 子对象字段与模型一致：redirect / gateway / auth_method 枚举。
    #[test]
    fn schema_subobject_fields_match_model() {
        let schema: Value = serde_json::from_str(profile_json_schema()).unwrap();
        let model = SessionProfile::default();

        let redirect_keys: Vec<_> = serde_json::to_value(&model.redirect)
            .unwrap().as_object().unwrap().keys().cloned().collect();
        let schema_redirect: Vec<_> = schema["properties"]["redirect"]["properties"]
            .as_object().unwrap().keys().cloned().collect();
        assert_eq!(redirect_keys, schema_redirect);

        // gateway 定义存在且含模型 gateway 序列化出的字段
        let gw = crate::profile::GatewayConfig::default();
        let gw_keys: Vec<_> = serde_json::to_value(&gw).unwrap()
            .as_object().unwrap().keys().cloned().collect();
        let schema_gw: Vec<_> = schema["definitions"]["gatewayConfig"]["properties"]
            .as_object().unwrap().keys().cloned().collect();
        assert_eq!(gw_keys, schema_gw);

        // auth_method 枚举
        let enums: Vec<String> = schema["properties"]["auth_method"]["enum"]
            .as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let model_enum = serde_json::to_value(model.auth_method).unwrap();
        assert!(enums.contains(&model_enum.as_str().unwrap().to_string()));
    }

    /// schema 的 port 约束与模型 u16 一致（1-65535）。
    #[test]
    fn schema_port_bounds_match() {
        let schema: Value = serde_json::from_str(profile_json_schema()).unwrap();
        assert_eq!(schema["properties"]["port"]["minimum"], json!(1));
        assert_eq!(schema["properties"]["port"]["maximum"], json!(65535));
    }

    /// schema 中 default 值与模型 Default 一致（抽样）。
    #[test]
    fn schema_defaults_match_model_default() {
        let schema: Value = serde_json::from_str(profile_json_schema()).unwrap();
        let model = serde_json::to_value(SessionProfile::default()).unwrap();
        assert_eq!(schema["properties"]["port"]["default"], model["port"]);
        assert_eq!(schema["properties"]["color_depth"]["default"], model["color_depth"]);
        assert_eq!(schema["properties"]["auth_method"]["default"], model["auth_method"]);
    }

    // ---------------- 校验器正向 ----------------

    #[test]
    fn valid_profile_passes_with_no_warnings() {
        let v = valid_profile();
        let warnings = validate_profile_json(&v, ValidationMode::Lenient).unwrap();
        assert!(warnings.is_empty());
        // 校验通过后应能被 serde 反序列化并过模型 validate
        let p: SessionProfile = serde_json::from_value(v).unwrap();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn full_profile_with_gateway_passes() {
        let v = json!({
            "name": "gw",
            "host": "10.0.0.5",
            "port": 3390,
            "username": "ops",
            "auth_method": "sso",
            "resolution": { "custom": { "w": 1920, "h": 1080 } },
            "color_depth": 24,
            "redirect": { "clipboard": true, "drives": true },
            "gateway": {
                "hostname": "gw.example.com",
                "port": 8443,
                "use_tls": false,
                "auth": { "type": "token" },
                "resource_id": "rdp-win01",
                "token": "jwt",
                "username": null,
                "password": null
            }
        });
        assert!(validate_profile_json(&v, ValidationMode::Lenient).is_ok());
        assert!(validate_profile_json(&v, ValidationMode::Strict).is_ok());
        let p: SessionProfile = serde_json::from_value(v).unwrap();
        assert!(p.validate().is_ok());
    }

    // ---------------- 校验器反向 ----------------

    #[test]
    fn missing_host_is_error_with_pointer() {
        let mut v = valid_profile();
        v.as_object_mut().unwrap().remove("host");
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#" && e.message.contains("host")), "{errs:?}");
    }

    #[test]
    fn empty_host_is_error() {
        let mut v = valid_profile();
        v["host"] = json!("   ");
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/host"), "{errs:?}");
    }

    #[test]
    fn port_as_string_is_type_error() {
        let mut v = valid_profile();
        v["port"] = json!("3389");
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].pointer, "#/port");
        assert!(errs[0].message.contains("类型错误"), "{}", errs[0].message);
    }

    #[test]
    fn port_out_of_range_is_error() {
        let mut v = valid_profile();
        v["port"] = json!(70000);
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/port" && e.message.contains("越界")));

        v["port"] = json!(0);
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/port"));
    }

    #[test]
    fn unknown_field_is_warning_in_lenient_error_in_strict() {
        let mut v = valid_profile();
        v["colorDepth"] = json!(32); // 未知字段（文档口径遗留命名）
        // 宽松：仅警告
        let warnings = validate_profile_json(&v, ValidationMode::Lenient).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].pointer, "#/colorDepth");
        assert_eq!(warnings[0].severity, Severity::Warning);
        // strict：失败且包含该警告
        let errs = validate_profile_json(&v, ValidationMode::Strict).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/colorDepth" && e.severity == Severity::Warning));
    }

    #[test]
    fn nested_gateway_field_errors() {
        let mut v = valid_profile();
        v["gateway"] = json!({
            "hostname": 123,
            "port": 0,
            "use_tls": "yes",
            "auth": { "type": "magic" }
        });
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        for ptr in ["#/gateway/hostname", "#/gateway/port", "#/gateway/use_tls", "#/gateway/auth/type"] {
            assert!(
                errs.iter().any(|e| e.pointer == ptr),
                "缺少 {ptr}: {errs:?}"
            );
        }
    }

    #[test]
    fn nested_gateway_unknown_field_warns() {
        let mut v = valid_profile();
        v["gateway"] = json!({ "hostname": "gw", "auth": { "type": "inherit" }, "extra": 1 });
        let warnings = validate_profile_json(&v, ValidationMode::Lenient).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].pointer, "#/gateway/extra");
    }

    #[test]
    fn redirect_wrong_type_is_error() {
        let mut v = valid_profile();
        v["redirect"] = json!({ "clipboard": "true" });
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/redirect/clipboard"), "{errs:?}");
    }

    #[test]
    fn invalid_auth_method_enum_is_error() {
        let mut v = valid_profile();
        v["auth_method"] = json!("nla"); // 文档口径的旧值，模型不存在
        let errs = validate_profile_json(&v, ValidationMode::Lenient).unwrap_err();
        assert!(errs.iter().any(|e| e.pointer == "#/auth_method"));
    }

    #[test]
    fn non_object_root_is_error() {
        let errs = validate_profile_json(&json!([1, 2]), ValidationMode::Lenient).unwrap_err();
        assert_eq!(errs[0].pointer, "#");
    }

    #[test]
    fn error_display_contains_pointer_and_message() {
        let i = err("#/host", "host 不能为空");
        assert_eq!(i.to_string(), "#/host: host 不能为空");
    }

    /// 与导出路径联动：export 产出的 JSON 必须过校验（无警告）。
    #[test]
    fn serialized_model_roundtrips_through_validator() {
        let mut p = SessionProfile::default();
        p.host = "srv.local".into();
        p.name = "rt".into();
        let v = serde_json::to_value(&p).unwrap();
        assert!(validate_profile_json(&v, ValidationMode::Strict).is_ok());
    }
}
