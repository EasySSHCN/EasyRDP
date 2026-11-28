//! --enroll：一次性 enrollment token 换取 AgentToken（方案 §6.1 形态 B）。
//!
//! 请求：`POST {gateway_url}/agent/enroll`，JSON `{"enrollment_token": "...", "resource_id": "..."}`
//! （Bearer 或 body 二选一，默认 body）。成功 200：`{"agent_token": "<JWT>"}`。
//! 失败（401/403/410）原样返回错误摘要，Agent 不落盘。
//!
//! 网关侧 enroll 端点在 P3/P4 服务器改造（动态资源注册表 + `/admin/resource`）
//! 一并交付（方案 §4）；Agent 侧按本模块约定实现，测试用内存传输层覆盖。

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EnrollRequest<'a> {
    pub enrollment_token: &'a str,
    pub resource_id: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub agent_token: String,
}

/// enroll 传输层抽象：真实实现走 HTTP（`http_transport`），
/// 测试用内存实现（`memory_transport`）。
pub type EnrollTransport<'a> =
    Box<dyn FnMut(&str, &[u8]) -> Result<(u16, Vec<u8>), String> + Send + 'a>;

/// 执行 enroll：返回网关签发的 AgentToken 明文（调用方负责 DPAPI 落盘）。
pub fn enroll(
    transport: &mut dyn FnMut(&str, &[u8]) -> Result<(u16, Vec<u8>), String>,
    gateway_url: &str,
    enrollment_token: &str,
    resource_id: &str,
) -> Result<String, String> {
    let body = serde_json::to_vec(&EnrollRequest {
        enrollment_token,
        resource_id,
    })
    .map_err(|e| format!("enroll 请求编码失败: {e}"))?;
    let endpoint = format!("{}/agent/enroll", gateway_url.trim_end_matches('/'));
    let (status, resp) = transport(&endpoint, &body)?;
    if status != 200 {
        let summary = String::from_utf8_lossy(&resp);
        let summary = summary.chars().take(200).collect::<String>();
        return Err(format!("enroll 被拒绝（HTTP {status}）: {summary}"));
    }
    let parsed: EnrollResponse =
        serde_json::from_slice(&resp).map_err(|e| format!("enroll 响应解析失败: {e}"))?;
    if parsed.agent_token.trim().is_empty() {
        return Err("enroll 响应缺少 agent_token".into());
    }
    Ok(parsed.agent_token)
}

/// 真实 HTTP 传输（明文 HTTP；生产部署网关前置 Nginx 终止 TLS，
/// 局域网 enroll 场景直接 HTTP 亦可，方案 §4 部署形态）。
pub fn http_transport() -> EnrollTransport<'static> {
    Box::new(|endpoint: &str, body: &[u8]| blocking_http_post(endpoint, body))
}

/// 无 TLS 依赖的极简阻塞 HTTP/1.1 POST（仅 http://；https 场景由前置反代承担）。
fn blocking_http_post(endpoint: &str, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
    use std::io::{Read, Write};

    let (host, port, path) = split_endpoint(endpoint)?;
    let mut stream =
        std::net::TcpStream::connect((host.as_str(), port)).map_err(|e| format!("连接网关失败: {e}"))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("发送 enroll 请求失败: {e}"))?;
    stream.write_all(body).map_err(|e| format!("发送 enroll body 失败: {e}"))?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| format!("读取 enroll 响应失败: {e}"))?;
    parse_http_response(&buf)
}

/// 拆 endpoint → (host, port, path)；http 默认 80，https 默认 443。
fn split_endpoint(endpoint: &str) -> Result<(String, u16, String), String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| format!("enroll endpoint 必须是 http(s) URL: {endpoint}"))?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return Err(format!("enroll endpoint scheme 不支持: {scheme}")),
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|_| format!("端口非法: {authority}"))?,
        ),
        None => (authority.to_string(), default_port),
    };
    Ok((host, port, format!("/{path}")))
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let text = String::from_utf8_lossy(raw);
    let split = text
        .find("\r\n\r\n")
        .ok_or_else(|| "HTTP 响应缺少头/体分隔".to_string())?;
    let status_line = text.lines().next().ok_or_else(|| "HTTP 响应为空".to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("HTTP 状态行非法: {status_line}"))?;
    Ok((status, raw[split + 4..].to_vec()))
}

/// 内存传输（单测）：按注册的 (status, body) 序列应答。
#[cfg(test)]
pub(crate) fn memory_transport(
    responses: Vec<Result<(u16, Vec<u8>), String>>,
) -> EnrollTransport<'static> {
    use std::sync::Mutex;
    let queue = Mutex::new(responses);
    Box::new(move |_endpoint: &str, _body: &[u8]| {
        let mut q = queue.lock().expect("queue");
        if q.is_empty() {
            return Err("memory transport 已耗尽".into());
        }
        q.remove(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_success_returns_agent_token() {
        let mut transport = memory_transport(vec![Ok((
            200,
            br#"{"agent_token":"eyJ.agent.jwt"}"#.to_vec(),
        ))]);
        let token = enroll(&mut *transport, "https://easyrdp.cn", "one-time-enroll", "u-ab12")
            .expect("enroll ok");
        assert_eq!(token, "eyJ.agent.jwt");
    }

    #[test]
    fn enroll_rejection_surfaces_status_and_summary() {
        let mut transport = memory_transport(vec![Ok((403, "enrollment token 已被使用".as_bytes().to_vec()))]);
        let err = enroll(&mut *transport, "https://easyrdp.cn/", "used-token", "u-ab12")
            .expect_err("应拒绝");
        assert!(err.contains("403"), "错误含状态码: {err}");
        assert!(err.contains("已被使用"), "错误含摘要: {err}");
    }

    #[test]
    fn enroll_transport_error_propagates() {
        let mut transport = memory_transport(vec![Err("连接失败".into())]);
        let err = enroll(&mut *transport, "http://gw:8443", "t", "r").expect_err("err");
        assert!(err.contains("连接失败"));
    }

    #[test]
    fn enroll_bad_json_and_empty_token_fail() {
        let mut transport = memory_transport(vec![Ok((200, b"not json".to_vec()))]);
        assert!(enroll(&mut *transport, "https://gw", "t", "r").is_err());
        let mut transport = memory_transport(vec![Ok((200, br#"{"agent_token":"  "#.to_vec()))]);
        assert!(enroll(&mut *transport, "https://gw", "t", "r").is_err());
    }

    #[test]
    fn request_endpoint_and_body_shape() {
        use std::sync::Mutex;
        let captured = Mutex::new(Vec::new());
        {
            let mut transport: EnrollTransport<'_> = Box::new(|endpoint: &str, body: &[u8]| {
                captured
                    .lock()
                    .expect("lock")
                    .push((endpoint.to_string(), body.to_vec()));
                Ok((200, br#"{"agent_token":"t"}"#.to_vec()))
            });
            enroll(&mut *transport, "https://easyrdp.cn/", "enroll-tok", "u-ab12").expect("ok");
        }
        let (ep, body) = captured.into_inner().expect("inner").remove(0);
        assert_eq!(ep, "https://easyrdp.cn/agent/enroll");
        let req: EnrollRequest = serde_json::from_slice(&body).expect("json");
        assert_eq!(req.enrollment_token, "enroll-tok");
        assert_eq!(req.resource_id, "u-ab12");
    }

    #[test]
    fn endpoint_split_defaults_and_explicit_ports() {
        assert_eq!(
            split_endpoint("http://gw:8443/agent/enroll").expect("ok"),
            ("gw".to_string(), 8443, "/agent/enroll".to_string())
        );
        assert_eq!(
            split_endpoint("http://gw").expect("ok"),
            ("gw".to_string(), 80, "/".to_string())
        );
        assert_eq!(
            split_endpoint("https://easyrdp.cn").expect("ok"),
            ("easyrdp.cn".to_string(), 443, "/".to_string())
        );
        assert!(split_endpoint("ftp://gw").is_err());
        assert!(split_endpoint("gw").is_err());
    }

    #[test]
    fn parse_http_response_splits_status_and_body() {
        let raw = b"HTTP/1.1 410 Gone\r\nContent-Type: text/plain\r\n\r\nexpired";
        let (status, body) = parse_http_response(raw).expect("parse");
        assert_eq!(status, 410);
        assert_eq!(body, b"expired");
        assert!(parse_http_response(b"garbage").is_err());
    }
}
