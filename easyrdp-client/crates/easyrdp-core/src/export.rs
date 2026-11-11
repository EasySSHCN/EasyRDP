//! SessionProfile → .rdp 文本导出，保证与 rdp_import 往返一致。

use crate::profile::{GatewayAuth, Redirect, Resolution, SessionProfile};

/// 导出为 .rdp 文本（UTF-8，CRLF 行尾，mstsc 兼容格式）。
pub fn export_to_string(p: &SessionProfile) -> String {
    let mut lines: Vec<String> = Vec::new();

    // full address 与端口
    if p.port == 3389 {
        lines.push(format!("full address:s:{}", p.host));
    } else {
        lines.push(format!("full address:s:{}:{}", p.host, p.port));
    }

    match &p.resolution {
        Resolution::Fullscreen => lines.push("screen mode id:i:1".into()),
        Resolution::FitWindow => lines.push("screen mode id:i:2".into()),
        Resolution::Custom { w, h } => {
            lines.push("screen mode id:i:2".into());
            lines.push(format!("desktopwidth:i:{w}"));
            lines.push(format!("desktopheight:i:{h}"));
        }
    }

    lines.push(format!("session bpp:i:{}", p.color_depth));
    if !p.username.is_empty() {
        lines.push(format!("username:s:{}", p.username));
    }

    push_redirect(&mut lines, "redirectclipboard", p.redirect.clipboard);
    push_redirect(&mut lines, "redirectprinters", p.redirect.printers);
    push_redirect(&mut lines, "redirectdrives", p.redirect.drives);
    push_redirect(&mut lines, "redirectcameras", p.redirect.camera);

    if let Some(gw) = &p.gateway {
        lines.push(format!("gatewayhostname:s:{}", gw.hostname));
        let source = match gw.auth {
            GatewayAuth::Inherit => "0",
            GatewayAuth::OtpCookie => "5",
        };
        lines.push(format!("gatewaycredentialssource:i:{source}"));
    }

    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

fn push_redirect(lines: &mut Vec<String>, key: &str, on: bool) {
    lines.push(format!("{key}:i:{}", if on { 1 } else { 0 }));
}

/// 写出到 .rdp 文件（UTF-8 编码）。
pub fn export_to_file(p: &SessionProfile, path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, export_to_string(p))
}

/// 导出 JSON（用于前端展示 / 持久化）。
pub fn export_json(p: &SessionProfile) -> String {
    serde_json::to_string_pretty(p).expect("SessionProfile 序列化不会失败")
}

/// Redirect 构造辅助（测试用）。
#[allow(dead_code)]
fn redirect_all(on: bool) -> Redirect {
    Redirect {
        clipboard: on,
        printers: on,
        drives: on,
        camera: on,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::GatewayConfig;
    use crate::rdp_import::parse_lines;

    #[test]
    fn roundtrip_import_export_import() {
        let source = "\
full address:s:rdp.corp.example.com:3390
screen mode id:i:2
desktopwidth:i:1920
desktopheight:i:1080
session bpp:i:24
username:s:CORP\\alice
redirectclipboard:i:1
redirectprinters:i:1
redirectdrives:i:0
redirectcameras:i:0
gatewayhostname:s:gw.corp.example.com
gatewaycredentialssource:i:5
";
        let lines: Vec<String> = source.lines().map(String::from).collect();
        let imported = parse_lines(&lines);

        let text = export_to_string(&imported);
        let re_lines: Vec<String> = text.lines().map(String::from).collect();
        let roundtripped = parse_lines(&re_lines);

        assert_eq!(imported, roundtripped);
    }

    #[test]
    fn roundtrip_default_profile_with_default_port() {
        let mut p = SessionProfile::default();
        p.host = "bare-host".into();
        p.name = "t".into();
        let text = export_to_string(&p);
        let re = parse_lines(&text.lines().map(String::from).collect::<Vec<_>>());
        assert_eq!(p.host, re.host);
        assert_eq!(p.port, re.port);
        assert_eq!(p, re);
    }

    #[test]
    fn exported_text_has_valid_key_type_value_lines() {
        let mut p = SessionProfile::default();
        p.host = "h".into();
        let text = export_to_string(&p);
        for line in text.lines() {
            let parts: Vec<&str> = line.splitn(3, ':').collect();
            assert_eq!(parts.len(), 3, "行不符合 key:type:value 格式: {line}");
            assert!(["s", "i"].contains(&parts[1]), "未知类型: {line}");
        }
    }

    #[test]
    fn export_json_is_valid() {
        let mut p = SessionProfile::default();
        p.host = "h".into();
        p.gateway = Some(GatewayConfig {
            hostname: "gw".into(),
            auth: GatewayAuth::Inherit,
        });
        let v: SessionProfile = serde_json::from_str(&export_json(&p)).unwrap();
        assert_eq!(v, p);
    }
}
