//! 解析 mstsc 导出的 .rdp 文本为 SessionProfile。
//!
//! .rdp 文件格式为 `key:type:value` 行（如 `full address:s:host`、
//! `screen mode id:i:2`、`redirect clipboard:i:1`），编码为 UTF-16LE（mstsc 默认）
//! 或 UTF-8。未知键一律忽略。

use crate::profile::{AuthMethod, GatewayAuth, GatewayConfig, Redirect, Resolution, SessionProfile};
use encoding_rs::UTF_16LE;
use std::path::Path;

/// 导入错误。
#[derive(Debug)]
pub enum ImportError {
    Io(std::io::Error),
    /// 既不是合法 UTF-16LE 也不是合法 UTF-8。
    BadEncoding,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Io(e) => write!(f, "读取文件失败: {e}"),
            ImportError::BadEncoding => write!(f, "文件既不是 UTF-16LE 也不是 UTF-8"),
        }
    }
}
impl std::error::Error for ImportError {}

/// 读取文件内容并自动识别 UTF-16LE / UTF-8，返回行列表。
pub fn read_lines(path: &Path) -> Result<Vec<String>, ImportError> {
    let raw = std::fs::read(path).map_err(ImportError::Io)?;
    let text = decode(&raw)?;
    Ok(text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// 解码字节：按 BOM / NUL 字节探测 UTF-16LE，否则按 UTF-8，最后回退 UTF-16LE 严格解码。
fn decode(raw: &[u8]) -> Result<String, ImportError> {
    // 1) 显式 BOM：FF FE = UTF-16LE；EF BB BF = UTF-8
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        return decode_le_strict(&raw[2..]);
    }
    if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        if let Ok(s) = String::from_utf8(raw[3..].to_vec()) {
            return Ok(s);
        }
    }
    // 2) 含 NUL 字节是 UTF-16 文本的典型特征（ASCII 文本不含 NUL）
    let utf16_likely = raw.len() % 2 == 0 && raw.chunks(2).any(|p| p[1] == 0);
    if utf16_likely {
        if let Ok(s) = decode_le_strict(raw) {
            return Ok(s);
        }
    }
    // 3) 先按 UTF-8（mstsc 亦可导出 UTF-8）
    if let Ok(s) = String::from_utf8(raw.to_vec()) {
        return Ok(s);
    }
    // 4) 回退：UTF-16LE 严格解码
    if raw.len() % 2 == 0 {
        if let Ok(s) = decode_le_strict(raw) {
            return Ok(s);
        }
    }
    Err(ImportError::BadEncoding)
}

/// 严格 UTF-16LE 解码（无 BOM 处理，任何非法单元即失败）。
fn decode_le_strict(raw: &[u8]) -> Result<String, ImportError> {
    let (cow, _, had_errors) = UTF_16LE.decode(raw);
    if had_errors {
        Err(ImportError::BadEncoding)
    } else {
        Ok(cow.into_owned())
    }
}

/// 解析单行 `key:type:value`。
/// 返回 (key, value)；解析失败或非数据行返回 None。
pub fn parse_line(line: &str) -> Option<(String, String)> {
    let (key, rest) = line.split_once(':')?;
    let (_ty, value) = rest.split_once(':')?;
    Some((key.trim().to_ascii_lowercase(), value.to_string()))
}

/// 把 .rdp 行列表解析为 SessionProfile。
pub fn parse_lines(lines: &[String]) -> SessionProfile {
    let mut p = SessionProfile::default();
    // screen mode id 是否出现过：出现则尊重其分辨率语义，desktopwidth/height 仅作补充
    let mut screen_mode_seen = false;

    for line in lines {
        let Some((key, value)) = parse_line(line) else {
            continue;
        };
        match key.as_str() {
            // full address:s:host:port —— host 与可选端口合并在同一值
            "full address" => {
                let v = value.trim_start_matches('*'); // mstsc 可能写 *Astxxx 掩码形式
                if let Some((host, port_str)) = v.rsplit_once(':') {
                    if let Ok(port) = port_str.parse::<u16>() {
                        if port != 0 {
                            p.port = port;
                            p.host = host.to_string();
                            continue;
                        }
                    }
                }
                p.host = v.to_string();
            }
            "server port" => {
                if let Ok(port) = value.parse::<u16>() {
                    if port != 0 {
                        p.port = port;
                    }
                }
            }
            "screen mode id" => {
                // i:1 = 全屏, i:2 = 窗口
                screen_mode_seen = true;
                p.resolution = match value.trim() {
                    "1" => Resolution::Fullscreen,
                    _ => Resolution::FitWindow,
                };
            }
            "desktopwidth" => {
                if let Ok(w) = value.parse::<u32>() {
                    if !screen_mode_seen {
                        p.resolution = Resolution::Custom { w, h: 0 };
                    }
                }
            }
            "desktopheight" => {
                if let Ok(h) = value.parse::<u32>() {
                    if !screen_mode_seen {
                        match p.resolution {
                            Resolution::Custom { w, .. } => p.resolution = Resolution::Custom { w, h },
                            _ => p.resolution = Resolution::Custom { w: 0, h },
                        }
                    }
                }
            }
            "session bpp" => {
                if let Ok(bpp) = value.parse::<u32>() {
                    p.color_depth = bpp;
                }
            }
            "name" => p.name = value, // EasyRDP 扩展键；mstsc 的未知键行为不受影响
            "username" => p.username = value,
            "redirectclipboard" => p.redirect.clipboard = value.trim() == "1",
            "redirectprinters" => p.redirect.printers = value.trim() == "1",
            "redirectdrives" => p.redirect.drives = value.trim() == "1",
            "redirectcameras" => p.redirect.camera = value.trim() == "1",
            "gatewayhostname" => {
                let gw = p.gateway.get_or_insert_with(|| GatewayConfig {
                    hostname: String::new(),
                    auth: GatewayAuth::Inherit,
                });
                gw.hostname = value;
            }
            "gatewaycredentialssource" => {
                let gw = p.gateway.get_or_insert_with(|| GatewayConfig {
                    hostname: String::new(),
                    auth: GatewayAuth::Inherit,
                });
                // mstsc: 0 = NTLM/继承凭据, 5 = 用户提供（此处映射为 OTP Cookie）
                gw.auth = match value.trim() {
                    "5" => GatewayAuth::OtpCookie,
                    _ => GatewayAuth::Inherit,
                };
            }
            // 未知键一律忽略
            _ => {}
        }
    }

    p
}

/// 从 .rdp 文件导入 SessionProfile。
pub fn import(path: &Path) -> Result<SessionProfile, ImportError> {
    let lines = read_lines(path)?;
    Ok(parse_lines(&lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn utf16le_bytes(s: &str) -> Vec<u8> {
        // 手动构造 UTF-16LE 字节（std 标准行为，不依赖第三方 encode 语义）
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn write_temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        // 优先 cargo OUT_DIR；不可用时回退到包内 test-tmp（避免依赖全局 %TEMP% 权限）
        let dir = std::env::var("OUT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-tmp");
                std::fs::create_dir_all(&p).unwrap();
                p
            });
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    const SAMPLE_UTF8: &str = "\
screen mode id:i:2
use multimon:i:1
desktopwidth:i:1920
desktopheight:i:1080
session bpp:i:32
winposstr:s:0,3,0,0,800,600
compression:i:1
keyboardhook:i:2
audiocapturemode:i:0
redirectprinters:i:1
redirectcomports:i:0
redirectsmartcards:i:1
redirectclipboard:i:1
redirectposconnections:i:1
drivestoredirect:s:
username:s:CORP\\alice
full address:s:rdp.corp.example.com
server port:i:3390
alternate shell:s:
shell working directory:s:
gatewayhostname:s:gw.corp.example.com
gatewaycredentialssource:i:5
";

    #[test]
    fn parse_utf8_sample() {
        let lines: Vec<String> = SAMPLE_UTF8.lines().map(String::from).collect();
        let p = parse_lines(&lines);
        assert_eq!(p.host, "rdp.corp.example.com");
        assert_eq!(p.port, 3390);
        assert_eq!(p.username, "CORP\\alice");
        assert_eq!(p.color_depth, 32);
        assert_eq!(p.resolution, Resolution::FitWindow);
        assert!(p.redirect.clipboard);
        assert!(p.redirect.printers);
        assert_eq!(
            p.gateway,
            Some(GatewayConfig {
                hostname: "gw.corp.example.com".into(),
                auth: GatewayAuth::OtpCookie,
            })
        );
    }

    #[test]
    fn utf16le_read_and_parse() {
        let path = write_temp("sample_utf16.rdp", &utf16le_bytes(SAMPLE_UTF8));
        let p = import(&path).unwrap();
        assert_eq!(p.host, "rdp.corp.example.com");
        assert_eq!(p.port, 3390);
        assert!(p.redirect.clipboard);
    }

    #[test]
    fn utf8_bom_is_tolerated() {
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(SAMPLE_UTF8.as_bytes());
        let path = write_temp("sample_bom.rdp", &bytes);
        let p = import(&path).unwrap();
        assert_eq!(p.host, "rdp.corp.example.com");
    }

    #[test]
    fn default_port_3389_when_absent() {
        let lines: Vec<String> = "full address:s:hostonly\r\n".lines().map(String::from).collect();
        let p = parse_lines(&lines);
        assert_eq!(p.host, "hostonly");
        assert_eq!(p.port, 3389);
    }

    #[test]
    fn full_address_with_port_suffix() {
        let lines: Vec<String> = "full address:s:myhost:3391\r\n".lines().map(String::from).collect();
        let p = parse_lines(&lines);
        assert_eq!(p.host, "myhost");
        assert_eq!(p.port, 3391);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let lines: Vec<String> = "\
somefuturekey:x:whatever
audiomode:i:2
full address:s:h1
"
        .lines()
        .map(String::from)
        .collect();
        let p = parse_lines(&lines);
        assert_eq!(p.host, "h1");
    }

    #[test]
    fn screen_mode_fullscreen() {
        let lines: Vec<String> = "screen mode id:i:1\r\n".lines().map(String::from).collect();
        let p = parse_lines(&lines);
        assert_eq!(p.resolution, Resolution::Fullscreen);
    }

    #[test]
    fn desktop_size_maps_to_custom_resolution() {
        let lines: Vec<String> = "desktopwidth:i:1280\r\ndesktopheight:i:720\r\n"
            .lines()
            .map(String::from)
            .collect();
        let p = parse_lines(&lines);
        assert_eq!(p.resolution, Resolution::Custom { w: 1280, h: 720 });
    }

    #[test]
    fn redirect_flags_zero_mean_false() {
        let lines: Vec<String> = "redirectclipboard:i:0\r\nredirectdrives:i:0\r\n"
            .lines()
            .map(String::from)
            .collect();
        let p = parse_lines(&lines);
        assert!(!p.redirect.clipboard);
        assert!(!p.redirect.drives);
        // 未出现的键保持默认
        assert!(!p.redirect.printers);
    }

    #[test]
    fn empty_and_malformed_lines_are_skipped() {
        let lines: Vec<String> = "\r\n\r\ngarbage-line-without-colons\r\nfull address:s:okhost\r\n"
            .lines()
            .map(String::from)
            .collect();
        let p = parse_lines(&lines);
        assert_eq!(p.host, "okhost");
    }

    #[test]
    fn parse_line_key_lowercased() {
        let (k, v) = parse_line("FULL ADDRESS:S:Host").unwrap();
        assert_eq!(k, "full address");
        assert_eq!(v, "Host");
    }

    #[test]
    fn missing_file_is_io_error() {
        let err = import(Path::new("Z:/no/such/file.rdp")).unwrap_err();
        assert!(matches!(err, ImportError::Io(_)));
    }
}
