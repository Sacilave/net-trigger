//! 静默 HTTP POST/GET 模拟登录客户端
//!
//! 核心设计：
//! 1. 纯后台静默报文发送，0 弹窗、0 夺取桌面焦点；
//! 2. 紧凑超时与微内存消耗（缓冲区硬限制 1KB）；
//! 3. 灵活支持 POST 表单编码、JSON 及 GET Query 传参；
//! 4. 自动集成动态宏替换（{username}, {password}, {ip}, {mac}, {time} 等）。

use crate::config::HttpAuthConfig;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
    pub success: bool,
    pub status_code: u16,
    pub message: String,
}

impl AuthResult {
    pub fn ok(status_code: u16, message: String) -> Self {
        Self {
            success: true,
            status_code,
            message,
        }
    }

    pub fn fail(status_code: u16, message: String) -> Self {
        Self {
            success: false,
            status_code,
            message,
        }
    }
}

/// 执行静默 HTTP 认证请求
pub fn execute_http_auth(
    config: &HttpAuthConfig,
    action_url: &str,
    params: &BTreeMap<String, String>,
) -> AuthResult {
    let method = config.method.to_uppercase();

    let agent = ureq::builder()
        .redirects(3) // 认证阶段允许跟随服务端的成功页面跳转 (如 302 -> success.html)
        .timeout_connect(Duration::from_millis(2500))
        .timeout_read(Duration::from_millis(3500))
        .build();

    let res = if method == "GET" {
        // 构建带有 Query 字符串的 GET 请求
        let mut query_parts = Vec::new();
        for (k, v) in params {
            query_parts.push(format!(
                "{}={}",
                urlencoding_encode(k),
                urlencoding_encode(v)
            ));
        }

        let full_url = if query_parts.is_empty() {
            action_url.to_string()
        } else if action_url.contains('?') {
            format!("{}&{}", action_url, query_parts.join("&"))
        } else {
            format!("{}?{}", action_url, query_parts.join("&"))
        };

        let mut req = agent.get(&full_url);
        for (k, v) in &config.headers {
            req = req.set(k, v);
        }
        req.call()
    } else {
        // 默认按 POST 提交
        let mut req = agent.post(action_url);
        for (k, v) in &config.headers {
            req = req.set(k, v);
        }

        let content_type = config
            .headers
            .get("Content-Type")
            .or_else(|| config.headers.get("content-type"))
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "application/x-www-form-urlencoded".to_string());

        if content_type.contains("json") {
            let json_body = match serde_json_to_string(params) {
                Ok(s) => s,
                Err(err) => {
                    return AuthResult::fail(400, format!("参数序列化 JSON 失败: {}", err));
                }
            };
            req.send_string(&json_body)
        } else {
            // application/x-www-form-urlencoded
            let mut form_pairs = Vec::new();
            for (k, v) in params {
                form_pairs.push(format!(
                    "{}={}",
                    urlencoding_encode(k),
                    urlencoding_encode(v)
                ));
            }
            let form_str = form_pairs.join("&");
            req.send_string(&form_str)
        }
    };

    match res {
        Ok(response) => {
            let code = response.status();
            // 读取少量响应正文进行诊断（最多 1KB，避免内存堆积）
            let mut buf = [0u8; 1024];
            let mut reader = response.into_reader().take(1024);
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);

            // 常见认证错误启发式检测
            let snippet_lower = snippet.to_lowercase();
            if snippet_lower.contains("password error")
                || snippet_lower.contains("err_pwd")
                || snippet_lower.contains("密码错误")
                || snippet_lower.contains("账号不存在")
                || snippet_lower.contains("欠费")
                || snippet_lower.contains("flow over")
                || snippet_lower.contains("\"result\":\"fail\"")
                || snippet_lower.contains("\"result\": \"fail\"")
                || snippet_lower.contains("原ip与当前用户不一致")
                || snippet_lower.contains("设备未注册")
            {
                AuthResult::fail(code, format!("网关拒绝认证: {}", snippet.trim()))
            } else {
                AuthResult::ok(code, format!("认证报文发送成功 (HTTP {})", code))
            }
        }
        Err(ureq::Error::Status(code, response)) => {
            let mut buf = [0u8; 512];
            let mut reader = response.into_reader().take(512);
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);
            AuthResult::fail(code, format!("网关返回 HTTP {} 错误: {}", code, snippet.trim()))
        }
        Err(ureq::Error::Transport(transport_err)) => {
            AuthResult::fail(0, format!("认证网络传输失败: {}", transport_err))
        }
    }
}

/// 极简零依赖的 URL 编码实现（避免额外引入 url/percent-encoding 冗余开销）
fn urlencoding_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

/// 极简键值对 JSON 序列化（零大型 json crate 依赖）
fn serde_json_to_string(map: &BTreeMap<String, String>) -> Result<String, std::fmt::Error> {
    use std::fmt::Write;
    let mut out = String::from("{");
    let mut first = true;
    for (k, v) in map {
        if !first {
            out.push(',');
        }
        first = false;
        write!(out, "\"{}\":\"{}\"", escape_json(k), escape_json(v))?;
    }
    out.push('}');
    Ok(out)
}

fn escape_json(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encoding() {
        assert_eq!(urlencoding_encode("hello world!"), "hello%20world%21");
        assert_eq!(urlencoding_encode("student@campus.edu"), "student%40campus.edu");
    }

    #[test]
    fn test_simple_json_serialize() {
        let mut map = BTreeMap::new();
        map.insert("user".to_string(), "admin".to_string());
        map.insert("pass".to_string(), "123\"456".to_string());
        let json = serde_json_to_string(&map).unwrap();
        assert!(json.contains("\"user\":\"admin\""));
        assert!(json.contains("\"pass\":\"123\\\"456\""));
    }
}
