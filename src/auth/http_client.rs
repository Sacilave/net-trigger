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
    let clean_url = action_url.trim();
    if clean_url.is_empty()
        || clean_url == "--url"
        || (!clean_url.starts_with("http://") && !clean_url.starts_with("https://"))
    {
        return AuthResult::fail(
            400,
            "登录接口网址 (action_url) 格式无效！请在设置中填入有效的 http:// 认证地址".to_string(),
        );
    }

    // 0. 若为锐捷 RG-SAM+ / eportal 校园网登录接口，走显式绑定局域网物理网卡的高速专线通道
    if clean_url.contains("InterFace.do") || clean_url.contains("eportal") {
        return execute_ruijie_sam_auth(clean_url, params, &config.headers);
    }

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

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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

/// 针对锐捷 RG-SAM+ / eportal 校园网的专属后台静默登录处理器
pub fn execute_ruijie_sam_auth(
    action_url: &str,
    params: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
) -> AuthResult {
    let username = params
        .get("userId")
        .or_else(|| params.get("username"))
        .cloned()
        .unwrap_or_default();

    if username.trim().is_empty() || username.contains("your_student_id") {
        return AuthResult::fail(
            400,
            "未配置校园网学号/账号！请右键托盘图标【设置】填入学号和密码即可静默登录。".to_string(),
        );
    }

    let password = params.get("password").cloned().unwrap_or_default();
    if password.trim().is_empty() || password.contains("your_password") {
        return AuthResult::fail(
            400,
            "未配置校园网密码！请右键托盘图标【设置】填入密码即可开启后台秒连。".to_string(),
        );
    }

    let query_string = params.get("queryString").cloned().unwrap_or_default();
    let service = params.get("service").cloned().unwrap_or_default();
    let operator_pwd = params.get("operatorPwd").cloned().unwrap_or_default();
    let operator_user_id = params.get("operatorUserId").cloned().unwrap_or_default();
    let validcode = params.get("validcode").cloned().unwrap_or_default();
    let password_encrypt = params
        .get("passwordEncrypt")
        .cloned()
        .unwrap_or_else(|| "false".to_string());

    // 锐捷 SAM+ 前端必须进行双重 URL 编码 (Double URL Encoding)
    fn enc2(input: &str) -> String {
        urlencoding_encode(&urlencoding_encode(input))
    }

    let body = format!(
        "userId={}&password={}&service={}&queryString={}&operatorPwd={}&operatorUserId={}&validcode={}&passwordEncrypt={}",
        enc2(&username),
        enc2(&password),
        enc2(&service),
        enc2(&query_string),
        enc2(&operator_pwd),
        enc2(&operator_user_id),
        enc2(&validcode),
        if password_encrypt == "true" { "true" } else { "false" }
    );

    let (host, port, path) = parse_url_components(action_url);

    // 优先采用显式绑定局域网物理网卡的专用 TCP 通道，杜绝双网卡冲突
    match send_bound_http_post(&host, port, &path, &body, headers) {
        Ok(resp_str) => {
            if resp_str.contains("\"result\":\"success\"")
                || resp_str.contains("\"result\": \"success\"")
                || resp_str.contains("用户在线")
                || resp_str.contains("已在线")
            {
                AuthResult::ok(200, "锐捷 SAM+ 校园网后台静默登录成功！".to_string())
            } else if resp_str.contains("\"result\":\"fail\"")
                || resp_str.contains("\"result\": \"fail\"")
            {
                let err_msg = extract_json_field(&resp_str, "message")
                    .unwrap_or_else(|| "认证失败，请检查账号密码".to_string());
                AuthResult::fail(200, format!("网关拒绝登录: {}", err_msg))
            } else {
                AuthResult::ok(200, "认证报文已发送".to_string())
            }
        }
        Err(_err) => {
            // 原生 Socket 若失败，降级走 ureq
            execute_ruijie_sam_ureq_fallback(action_url, &body, headers)
        }
    }
}

fn execute_ruijie_sam_ureq_fallback(
    action_url: &str,
    body: &str,
    headers: &BTreeMap<String, String>,
) -> AuthResult {
    let agent = ureq::builder()
        .redirects(0)
        .timeout_connect(Duration::from_millis(2500))
        .timeout_read(Duration::from_millis(3500))
        .build();

    let mut req = agent.post(action_url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let res = req.send_string(body);

    match res {
        Ok(response) => {
            let mut buf = [0u8; 1024];
            let mut reader = response.into_reader().take(1024);
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);
            if snippet.contains("\"result\":\"success\"")
                || snippet.contains("\"result\": \"success\"")
                || snippet.contains("用户在线")
                || snippet.contains("已在线")
            {
                AuthResult::ok(200, "锐捷 SAM+ 校园网静默登录成功 (备用通道)".to_string())
            } else if snippet.contains("\"result\":\"fail\"") || snippet.contains("\"result\": \"fail\"") {
                let err_msg = extract_json_field(&snippet, "message").unwrap_or_else(|| "认证失败".to_string());
                AuthResult::fail(200, format!("网关拒绝登录: {}", err_msg))
            } else {
                AuthResult::ok(200, "认证报文已发送".to_string())
            }
        }
        Err(err) => AuthResult::fail(0, format!("网关网络请求失败: {}", err)),
    }
}

/// 解析 URL 中的 host, port, path
fn parse_url_components(url: &str) -> (String, u16, String) {
    let clean = if let Some(stripped) = url.strip_prefix("http://") {
        stripped
    } else if let Some(stripped) = url.strip_prefix("https://") {
        stripped
    } else {
        url
    };

    let (host_port, path) = match clean.find('/') {
        Some(pos) => (&clean[..pos], &clean[pos..]),
        None => (clean, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(pos) => (
            host_port[..pos].to_string(),
            host_port[pos + 1..].parse::<u16>().unwrap_or(80),
        ),
        None => (host_port.to_string(), 80),
    };

    (host, port, path.to_string())
}

/// 发送绑定到局域网物理网卡的原生 HTTP POST 请求
fn send_bound_http_post(
    host: &str,
    port: u16,
    path: &str,
    body: &str,
    headers: &BTreeMap<String, String>,
) -> Result<String, String> {
    use std::io::{Read, Write};

    let mut stream = connect_lan_bound_tcp(host, port, 3000)
        .map_err(|e| format!("连接网关服务器失败 ({}:{}): {}", host, port, e))?;

    let mut req_str = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        path,
        host,
        body.len()
    );

    let mut has_content_type = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        req_str.push_str(&format!("{}: {}\r\n", k, v));
    }
    if !has_content_type {
        req_str.push_str("Content-Type: application/x-www-form-urlencoded; charset=UTF-8\r\n");
    }
    req_str.push_str("\r\n");
    req_str.push_str(body);

    stream
        .write_all(req_str.as_bytes())
        .map_err(|e| format!("向网关发送认证报文失败: {}", e))?;

    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        response_bytes.extend_from_slice(&buf[..n]);
        if response_bytes.len() > 16384 {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&response_bytes).to_string())
}

#[cfg(windows)]
fn connect_lan_bound_tcp(
    remote_ip: &str,
    port: u16,
    timeout_ms: u32,
) -> std::io::Result<std::net::TcpStream> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::*;

    let local_lan_ip = crate::config::get_lan_adapter_ipv4();
    let remote_v4 = remote_ip
        .parse::<std::net::Ipv4Addr>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    unsafe {
        let sock = socket(AF_INET as i32, SOCK_STREAM, IPPROTO_TCP);
        if sock == !0 {
            return Err(std::io::Error::last_os_error());
        }

        let tv = timeout_ms;
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &tv as *const _ as *const _,
            std::mem::size_of_val(&tv) as i32,
        );
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_SNDTIMEO,
            &tv as *const _ as *const _,
            std::mem::size_of_val(&tv) as i32,
        );

        if let Some(ref lip) = local_lan_ip {
            if let Ok(local_v4) = lip.parse::<std::net::Ipv4Addr>() {
                let lo = local_v4.octets();
                let local_sin = SOCKADDR_IN {
                    sin_family: AF_INET as u16,
                    sin_port: 0,
                    sin_addr: IN_ADDR {
                        S_un: IN_ADDR_0 {
                            S_addr: u32::from_ne_bytes(lo),
                        },
                    },
                    sin_zero: [0; 8],
                };
                bind(
                    sock,
                    &local_sin as *const _ as *const _,
                    std::mem::size_of_val(&local_sin) as i32,
                );
            }
        }

        let ro = remote_v4.octets();
        let remote_sin = SOCKADDR_IN {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    S_addr: u32::from_ne_bytes(ro),
                },
            },
            sin_zero: [0; 8],
        };

        let ret = connect(
            sock,
            &remote_sin as *const _ as *const _,
            std::mem::size_of_val(&remote_sin) as i32,
        );

        if ret != 0 {
            let err = std::io::Error::last_os_error();
            closesocket(sock);
            return Err(err);
        }

        Ok(std::net::TcpStream::from_raw_socket(sock as _))
    }
}

#[cfg(not(windows))]
fn connect_lan_bound_tcp(
    remote_ip: &str,
    port: u16,
    timeout_ms: u32,
) -> std::io::Result<std::net::TcpStream> {
    use std::time::Duration;
    let addr = format!("{}:{}", remote_ip, port)
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms as u64))
}

fn extract_json_field(json_str: &str, field_name: &str) -> Option<String> {
    let key = format!("\"{}\":", field_name);
    let key_space = format!("\"{}\" :", field_name);
    let start_pos = json_str
        .find(&key)
        .map(|p| p + key.len())
        .or_else(|| json_str.find(&key_space).map(|p| p + key_space.len()))?;

    let rest = json_str[start_pos..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end_pos = stripped.find('"')?;
        Some(stripped[..end_pos].to_string())
    } else {
        None
    }
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

    #[test]
    fn test_invalid_action_url() {
        let cfg = HttpAuthConfig {
            method: "POST".to_string(),
            action_url: "--url".to_string(),
            params: BTreeMap::new(),
            headers: BTreeMap::new(),
        };
        let res = execute_http_auth(&cfg, "--url", &BTreeMap::new());
        assert!(!res.success);
        assert!(res.message.contains("格式无效"));
    }

    #[test]
    fn test_ruijie_sam_empty_credentials() {
        let mut params = BTreeMap::new();
        params.insert("userId".to_string(), "2311611043".to_string());
        // No password
        let res = execute_ruijie_sam_auth("http://10.10.200.102/eportal/InterFace.do?method=login", &params, &BTreeMap::new());
        assert!(!res.success);
        assert!(res.message.contains("未配置校园网密码"));
    }
}
