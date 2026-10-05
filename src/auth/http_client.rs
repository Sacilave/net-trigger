//! 静默 HTTP POST/GET 模拟登录客户端
//!
//! 核心设计：
//! 1. 纯后台静默报文发送，0 弹窗、0 夺取桌面焦点；
//! 2. 紧凑超时与微内存消耗（缓冲区硬限制 1KB）；
//! 3. 灵活支持 POST 表单编码、JSON 及 GET Query 传参；
//! 4. 自动集成动态宏替换（{username}, {password}, {ip}, {mac}, {time} 等）；
//! 5. 模块化解耦：私有协议驱动（如锐捷 SAM+）独立拆解至专用子模块。

use crate::config::HttpAuthConfig;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

// 重新导出锐捷专用驱动，维持 API 兼容性
pub use crate::auth::ruijie::{
    execute_ruijie_sam_auth, get_fresh_ruijie_portal_url, get_fresh_ruijie_query_string,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
    pub success: bool,
    pub status_code: u16,
    pub message: String,
    pub stage_code: &'static str,
    pub suggestion: String,
}

impl AuthResult {
    pub fn ok(status_code: u16, message: String) -> Self {
        Self {
            success: true,
            status_code,
            message,
            stage_code: "OK",
            suggestion: String::new(),
        }
    }

    pub fn fail(status_code: u16, message: String) -> Self {
        Self {
            success: false,
            status_code,
            message,
            stage_code: "FAIL",
            suggestion: String::new(),
        }
    }

    pub fn fail_with_stage(
        status_code: u16,
        message: String,
        stage_code: &'static str,
        suggestion: String,
    ) -> Self {
        Self {
            success: false,
            status_code,
            message,
            stage_code,
            suggestion,
        }
    }
}

/// 执行通用静默 HTTP 认证请求
pub fn execute_http_auth(
    config: &HttpAuthConfig,
    action_url: &str,
    params: &BTreeMap<String, String>,
) -> AuthResult {
    let clean_url = action_url.trim();
    if clean_url.is_empty()
        || clean_url == "--url"
        || (!clean_url.starts_with("http://") && !clean_url.starts_with("https://"))
    {
        return AuthResult::fail_with_stage(
            400,
            "登录接口网址 (action_url) 格式无效！".to_string(),
            "E4-01",
            "请在设置中填入有效的 http:// 或 https:// 认证地址".to_string(),
        );
    }

    // 0. 若为锐捷 RG-SAM+ / eportal 校园网登录接口，派发至锐捷专用驱动处理
    if clean_url.contains("InterFace.do") || clean_url.contains("eportal") {
        return execute_ruijie_sam_auth(clean_url, params, &config.headers, None);
    }

    let method = config.method.to_uppercase();

    let agent = ureq::builder()
        .redirects(3) // 认证阶段允许跟随服务端的成功页面跳转 (如 302 -> success.html)
        .timeout_connect(Duration::from_millis(2500))
        .timeout_read(Duration::from_millis(3500))
        .build();

    let res = if method == "GET" {
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
                    return AuthResult::fail_with_stage(
                        400,
                        format!("参数序列化 JSON 失败: {}", err),
                        "E4-01",
                        "请检查自定义参数格式是否符合 JSON 规范".to_string(),
                    );
                }
            };
            req.send_string(&json_body)
        } else {
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
            let mut buf = [0u8; 1024];
            let mut reader = response.into_reader().take(1024);
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);

            let snippet_lower = snippet.to_lowercase();
            if snippet_lower.contains("password error")
                || snippet_lower.contains("err_pwd")
                || snippet_lower.contains("密码错误")
                || snippet_lower.contains("账号不存在")
                || snippet_lower.contains("欠费")
                || snippet_lower.contains("flow over")
                || snippet_lower.contains("\"result\":\"fail\"")
                || snippet_lower.contains("\"result\": \"fail\"")
            {
                let (stage, sug) = crate::auth::ruijie::classify_ruijie_error(&snippet);
                AuthResult::fail_with_stage(code, format!("网关拒绝认证: {}", snippet.trim()), stage, sug)
            } else {
                AuthResult::ok(code, format!("认证报文发送成功 (HTTP {})", code))
            }
        }
        Err(ureq::Error::Status(code, response)) => {
            let mut buf = [0u8; 512];
            let mut reader = response.into_reader().take(512);
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);
            AuthResult::fail_with_stage(
                code,
                format!("网关返回 HTTP {} 错误: {}", code, snippet.trim()),
                "E5-02",
                "认证服务器返回异常，服务器系统可能在维护或接口路径错误。".to_string(),
            )
        }
        Err(ureq::Error::Transport(transport_err)) => {
            AuthResult::fail_with_stage(
                0,
                format!("认证网络传输失败: {}", transport_err),
                "E3-01",
                "无法连接到认证服务器，请检查 Wi-Fi 连接或本地局域网路由。".to_string(),
            )
        }
    }
}

/// 极简零依赖的 URL 编码实现
pub fn urlencoding_encode(input: &str) -> String {
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

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// 极简键值对 JSON 序列化
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding_encode() {
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
        assert_eq!(urlencoding_encode("abc-123_.~"), "abc-123_.~");
    }

    #[test]
    fn test_serde_json_to_string() {
        let mut map = BTreeMap::new();
        map.insert("key".to_string(), "val".to_string());
        let json = serde_json_to_string(&map).unwrap();
        assert_eq!(json, "{\"key\":\"val\"}");
    }
}
