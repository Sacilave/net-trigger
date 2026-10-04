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
        return AuthResult::fail_with_stage(
            400,
            "登录接口网址 (action_url) 格式无效！".to_string(),
            "E4-01",
            "请在设置中填入有效的 http:// 或 https:// 认证地址".to_string(),
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
                let (stage, sug) = classify_ruijie_error(&snippet);
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

/// 从局域网物理网卡向指定外网 IP 发起轻量探测，捕获校园网 AC 硬件返回的 302 重定向认证地址
pub fn fetch_portal_redirect_via_lan(target_ip: &str) -> Option<String> {
    use std::io::{Read, Write};
    let mut stream = connect_lan_bound_tcp(target_ip, 80, 2000).ok()?;
    let req_str = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\nConnection: close\r\n\r\n",
        target_ip
    );
    stream.write_all(req_str.as_bytes()).ok()?;

    let mut response_bytes = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        response_bytes.extend_from_slice(&buf[..n]);
        if response_bytes.len() > 8192 {
            break;
        }
    }

    let resp_str = String::from_utf8_lossy(&response_bytes);

    // 1. 优先提取 HTTP 30x Location 响应头 (包含当前动态 wlanuserip 与 nasip 等)
    for line in resp_str.lines() {
        let trimmed = line.trim();
        let loc_opt = if let Some(loc) = trimmed.strip_prefix("Location: ") {
            Some(loc.trim())
        } else if let Some(loc) = trimmed.strip_prefix("location: ") {
            Some(loc.trim())
        } else {
            None
        };
        if let Some(loc) = loc_opt {
            // 过滤外网直接访问 Cloudflare 等非劫持的跳转
            if loc.starts_with("https://1.1.1.1") || loc.starts_with("http://1.1.1.1") {
                continue;
            }
            return Some(loc.to_string());
        }
    }

    // 2. 备选提取 HTML 中的重定向脚本/标签: location.href="http://..." 或 <meta url=...>
    if let Some(pos) = resp_str.find("http://") {
        let remainder = &resp_str[pos..];
        let end_idx = remainder
            .find(|c| c == '\'' || c == '"' || c == '\r' || c == '\n' || c == '<' || c == '>')
            .unwrap_or(remainder.len());
        let candidate = &remainder[..end_idx];
        if candidate.contains("eportal") || candidate.contains("index.jsp") {
            return Some(candidate.to_string());
        }
    }

    None
}

/// 实时探测局域网并获取最新的动态 queryString (如 wlanuserip=...&wlanacname=...)
pub fn get_fresh_ruijie_query_string() -> Option<String> {
    for target in &["1.1.1.1", "123.123.123.123"] {
        if let Some(url) = fetch_portal_redirect_via_lan(target) {
            if let Some(q_pos) = url.find('?') {
                let qs = &url[q_pos + 1..];
                if !qs.is_empty() {
                    return Some(qs.to_string());
                }
            }
        }
    }
    None
}

/// 实时探测局域网并获取最新的动态登录页面 URL
pub fn get_fresh_ruijie_portal_url() -> Option<String> {
    for target in &["1.1.1.1", "123.123.123.123"] {
        if let Some(url) = fetch_portal_redirect_via_lan(target) {
            if url.contains("eportal") || url.contains("index.jsp") {
                return Some(url);
            }
        }
    }
    None
}

/// 锐捷 SAM+ queryString 专用编码：仅替换 & 为 %2526、= 为 %253D
///
/// 锐捷 ePortal 的 queryString 不是标准全量 URL 编码！
/// 浏览器 JS 实际执行的是 encodeURIComponent(encodeURIComponent(qs))，
/// 但因为 qs 内容本身只含 [a-zA-Z0-9&=] 和已编码的 hex，
/// 两次 encodeURIComponent 的实际效果等价于：& → %2526, = → %253D，其余字符保持原样。
pub fn encode_ruijie_query_string(raw_qs: &str) -> String {
    raw_qs.replace('&', "%2526").replace('=', "%253D")
}

/// 从 URL 查询字符串中提取指定参数值
fn extract_query_param(qs: &str, param_name: &str) -> Option<String> {
    let prefix = format!("{}=", param_name);
    for part in qs.split('&') {
        if let Some(val) = part.strip_prefix(&prefix) {
            return Some(val.to_string());
        }
    }
    None
}

/// 锐捷 SAM+ 标准 RSA 算法加密密码
///
/// 遵循锐捷 security.js / login_bch.js 官方规范：
/// 1. 待加密内容为：`password + ">" + mac`
/// 2. 字符串反转：`(password + ">" + mac).chars().rev().collect()`
/// 3. 分块按 16 位小端字打包为 BigUint：chunkSize = 2 * (digits - 1)
/// 4. 模幂运算：`block.modpow(e, m)`
/// 5. 按 16 位大端字格式化为十六进制串（每字 4 个 hex 字符）
pub fn rsa_encrypt_ruijie(plain: &str, exponent_hex: &str, modulus_hex: &str) -> Option<String> {
    use num_bigint::BigUint;
    use num_traits::Num;

    let e = BigUint::from_str_radix(exponent_hex, 16).ok()?;
    let m = BigUint::from_str_radix(modulus_hex, 16).ok()?;

    let m_bytes = m.to_bytes_be();
    let num_digits_16 = (m_bytes.len() + 1) / 2;
    let high_index = num_digits_16.saturating_sub(1);
    let chunk_size = 2 * high_index;
    if chunk_size == 0 {
        return None;
    }

    let mut a: Vec<u8> = plain.bytes().collect();
    while a.len() % chunk_size != 0 {
        a.push(0);
    }

    let mut result = String::new();
    for chunk in a.chunks(chunk_size) {
        let block = BigUint::from_bytes_le(chunk);
        let crypt = block.modpow(&e, &m);

        let crypt_bytes = crypt.to_bytes_le();
        let mut words_16 = Vec::with_capacity(num_digits_16);
        for i in 0..num_digits_16 {
            let low = crypt_bytes.get(2 * i).copied().unwrap_or(0) as u16;
            let high = crypt_bytes.get(2 * i + 1).copied().unwrap_or(0) as u16;
            words_16.push(low | (high << 8));
        }

        let mut hi = words_16.len().saturating_sub(1);
        while hi > 0 && words_16[hi] == 0 {
            hi -= 1;
        }

        let mut hex_chunk = String::new();
        for i in (0..=hi).rev() {
            use std::fmt::Write;
            write!(&mut hex_chunk, "{:04x}", words_16[i]).ok();
        }

        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(&hex_chunk);
    }

    Some(result)
}

/// 查询锐捷 SAM+ 网关页面配置（获取是否开启 RSA 密码加密及公钥指数和模数）
fn fetch_ruijie_page_info(host: &str, port: u16, query_string: &str) -> Option<(String, String)> {
    let mut headers = BTreeMap::new();
    headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded; charset=UTF-8".to_string());
    headers.insert("Referer".to_string(), format!("http://{}/eportal/index.jsp?{}", host, query_string));
    headers.insert("Origin".to_string(), format!("http://{}", host));
    headers.insert("Accept".to_string(), "application/json, text/javascript, */*; q=0.01".to_string());
    headers.insert("X-Requested-With".to_string(), "XMLHttpRequest".to_string());
    headers.insert("User-Agent".to_string(), "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

    let body = format!("queryString={}", urlencoding_encode(query_string));
    let resp = send_bound_http_post(host, port, "/eportal/InterFace.do?method=pageInfo", &body, &headers).ok()?;

    if resp.contains("\"passwordEncrypt\":\"true\"") || resp.contains("\"passwordEncrypt\": \"true\"") {
        let exp = extract_json_field(&resp, "publicKeyExponent").unwrap_or_else(|| "10001".to_string());
        let modulus = extract_json_field(&resp, "publicKeyModulus")?;
        return Some((exp, modulus));
    }
    None
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
        return AuthResult::fail_with_stage(
            400,
            "未配置校园网学号/账号！".to_string(),
            "E4-01",
            "请右键托盘图标【设置】填入学号和密码即可静默登录。".to_string(),
        );
    }

    let password = params.get("password").cloned().unwrap_or_default();
    if password.trim().is_empty() || password.contains("your_password") {
        return AuthResult::fail_with_stage(
            400,
            "未配置校园网密码！".to_string(),
            "E4-01",
            "请右键托盘图标【设置】填入密码即可开启后台秒连。".to_string(),
        );
    }

    let mut query_string = String::new();

    // 核心策略：始终优先通过物理局域网网卡实时捕获当前 Wi-Fi 会话的最新 queryString
    if let Some(fresh_qs) = get_fresh_ruijie_query_string() {
        query_string = fresh_qs;
    }

    // 动态获取失败时，降级使用传入参数中的 queryString（来自探针捕获或配置文件）
    if query_string.is_empty() {
        if let Some(qs_param) = params.get("queryString") {
            let mut qs = qs_param.clone();
            if let Some(q_pos) = qs.find('?') {
                qs = qs[q_pos + 1..].to_string();
            }
            if !qs.trim().is_empty() {
                query_string = qs;
            }
        }
    }

    // 最后兜底：从 action_url 本身提取
    if query_string.is_empty() {
        if let Some(q_pos) = action_url.find('?') {
            let qs = &action_url[q_pos + 1..];
            if !qs.is_empty() {
                query_string = qs.to_string();
            }
        }
    }

    if query_string.is_empty() {
        return AuthResult::fail_with_stage(
            400,
            "无法获取校园网认证参数 (queryString 为空)".to_string(),
            "E2-02",
            "未能从 Wi-Fi 网络捕获到 wlanuserip 会话参数。请确认已连接 usywireless 校园网热点。".to_string(),
        );
    }

    let (host, port, path) = parse_url_components(action_url);

    // 关键特性：智能识别网关是否开启了 RSA 密码加密，自动计算公钥密文
    let (pwd_to_send, encrypt_flag) = if let Some((exp, modulus)) = fetch_ruijie_page_info(&host, port, &query_string) {
        let mac_str = extract_query_param(&query_string, "mac").unwrap_or_else(|| "111111111".to_string());
        let mac_str = if mac_str.is_empty() { "111111111".to_string() } else { mac_str };
        let plain = format!("{}>{}", password, mac_str);
        let reversed: String = plain.chars().rev().collect();
        if let Some(encrypted) = rsa_encrypt_ruijie(&reversed, &exp, &modulus) {
            (encrypted, "true".to_string())
        } else {
            (password.clone(), "false".to_string())
        }
    } else {
        (password.clone(), "false".to_string())
    };

    let service = params.get("service").cloned().unwrap_or_default();
    let operator_pwd = params.get("operatorPwd").cloned().unwrap_or_default();
    let operator_user_id = params.get("operatorUserId").cloned().unwrap_or_default();
    let validcode = params.get("validcode").cloned().unwrap_or_default();

    // 锐捷 SAM+ 规范：
    // 1. queryString 必须采用特殊替换编码 (& -> %2526, = -> %253D)，绝不可对其全量二次 URL 编码！
    // 2. 其余字段按标准 URL 编码一次传输，保证特殊字符安全传输
    let enc_username = urlencoding_encode(&username);
    let enc_password = urlencoding_encode(&pwd_to_send);
    let enc_service = urlencoding_encode(&service);
    let enc_query_string = encode_ruijie_query_string(&query_string);
    let enc_operator_pwd = urlencoding_encode(&operator_pwd);
    let enc_operator_user_id = urlencoding_encode(&operator_user_id);
    let enc_validcode = urlencoding_encode(&validcode);
    let enc_encrypt = urlencoding_encode(&encrypt_flag);

    let body = format!(
        "userId={}&password={}&service={}&queryString={}&operatorPwd={}&operatorUserId={}&validcode={}&passwordEncrypt={}",
        enc_username,
        enc_password,
        enc_service,
        enc_query_string,
        enc_operator_pwd,
        enc_operator_user_id,
        enc_validcode,
        enc_encrypt
    );

    // 关键特性：精确补全锐捷 ePortal 所需的浏览器级请求头（缺少 Referer 网关会直接拒绝认证）
    let mut req_headers = headers.clone();
    req_headers.insert("Content-Type".to_string(), "application/x-www-form-urlencoded; charset=UTF-8".to_string());
    req_headers.insert("Referer".to_string(), format!("http://{}/eportal/index.jsp?{}", host, query_string));
    req_headers.insert("Origin".to_string(), format!("http://{}", host));
    req_headers.insert("Accept".to_string(), "application/json, text/javascript, */*; q=0.01".to_string());
    req_headers.insert("X-Requested-With".to_string(), "XMLHttpRequest".to_string());
    req_headers.entry("User-Agent".to_string()).or_insert_with(|| {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string()
    });

    // 优先采用显式绑定局域网物理网卡的专用 TCP 通道，杜绝双网卡冲突
    let result = match send_bound_http_post(&host, port, &path, &body, &req_headers) {
        Ok(resp_str) => parse_ruijie_response(&resp_str),
        Err(err) => {
            // 原生 Socket 若失败，降级走 ureq
            let ureq_res = execute_ruijie_sam_ureq_fallback(action_url, &body, &req_headers);
            if !ureq_res.success && ureq_res.status_code == 0 {
                AuthResult::fail_with_stage(
                    0,
                    format!("网关不可达 ({}:{})", host, port),
                    "E3-01",
                    format!("无法与校园网认证服务器 ({}:{}) 建立 TCP 连接：{}。请检查 Wi-Fi 是否连接正常。", host, port, err),
                )
            } else {
                ureq_res
            }
        }
    };

    if result.success {
        return result;
    }

    // 自愈重试：若网关报"原ip与当前用户不一致"或"设备未注册"，说明 queryString 过期
    if result.message.contains("原ip") || result.message.contains("设备未注册") || result.message.contains("queryString") || result.message.contains("参数") || result.message.contains("为空") {
        if let Some(retry_qs) = get_fresh_ruijie_query_string() {
            if retry_qs != query_string {
                return execute_ruijie_sam_auth(action_url, params, headers);
            }
        }
    }

    result
}

/// 锐捷 SAM+ 错误信息细粒度归类与排障建议映射
pub fn classify_ruijie_error(msg: &str) -> (&'static str, String) {
    let lower = msg.to_lowercase();
    if lower.contains("密码错误")
        || lower.contains("用户不存在")
        || lower.contains("password")
        || lower.contains("err_pwd")
        || lower.contains("密码不正确")
    {
        (
            "E5-01",
            "请核对学号与密码是否完全正确（注意大小写）。若曾修改过密码，请在设置中更新。".to_string(),
        )
    } else if lower.contains("原ip") || lower.contains("ip与当前用户不一致") {
        (
            "E5-01",
            "校园网会话 IP 与认证参数不一致。NetTrigger 会自动重新捕获最新会话参数，或可尝试重新连接 Wi-Fi。".to_string(),
        )
    } else if lower.contains("超限") || lower.contains("数量超限") || lower.contains("max") {
        (
            "E5-01",
            "该学号当前在线设备数量已达上限。请在手机或其他已登录设备上注销下线，或登录自服务系统踢出旧设备。".to_string(),
        )
    } else if lower.contains("欠费") || lower.contains("余额不足") || lower.contains("停机") || lower.contains("arrear") {
        (
            "E5-01",
            "校园网账号可能欠费或停机。请登录校园网统一结算门户或自服务系统查询充值。".to_string(),
        )
    } else if lower.contains("设备未注册") || lower.contains("mac") || lower.contains("绑定") {
        (
            "E5-01",
            "网关提示该设备或 MAC 地址未绑定。请登录校园网自服务门户完成设备绑定，或联系网络中心。".to_string(),
        )
    } else if lower.contains("验证码") || lower.contains("validcode") {
        (
            "E5-01",
            "网关当前开启了图形验证码校验。静默后台登录不支持输入验证码，请切换为【自动打开网页登录】模式。".to_string(),
        )
    } else {
        (
            "E5-01",
            format!("网关返回报错：{}。请根据提示在【设置】中核对配置或联系校园网管理员。", msg),
        )
    }
}

/// 解析锐捷 SAM+ 网关响应 JSON
fn parse_ruijie_response(resp_str: &str) -> AuthResult {
    if resp_str.contains("\"result\":\"success\"")
        || resp_str.contains("\"result\": \"success\"")
        || resp_str.contains("用户在线")
        || resp_str.contains("已在线")
    {
        AuthResult::ok(200, "锐捷 SAM+ 校园网后台静默登录成功！".to_string())
    } else if resp_str.contains("\"result\":\"fail\"")
        || resp_str.contains("\"result\": \"fail\"")
        || resp_str.contains("\"result\":\"error\"")
    {
        let raw_msg = extract_json_field(resp_str, "message")
            .unwrap_or_else(|| "认证失败，请检查账号密码".to_string());
        let (stage_code, suggestion) = classify_ruijie_error(&raw_msg);
        AuthResult::fail_with_stage(
            200,
            format!("网关拒绝登录: {}", raw_msg),
            stage_code,
            suggestion,
        )
    } else if resp_str.contains("HTTP/1.1 500") || resp_str.contains("HTTP/1.1 502") || resp_str.contains("HTTP/1.1 503") {
        AuthResult::fail_with_stage(
            500,
            "网关服务器返回 5xx 异常".to_string(),
            "E5-02",
            "校园网认证服务器内部错误，学校 SAM+ 系统可能在维护中，稍后将自动重试。".to_string(),
        )
    } else {
        AuthResult::fail_with_stage(
            200,
            format!("网关返回未识别内容: {}", resp_str.chars().take(60).collect::<String>()),
            "E5-01",
            "网关响应中未包含登录成功标记，请检查学号与密码，或在【设置】中切换为自动打开网页模式。".to_string(),
        )
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
            parse_ruijie_response(&snippet)
        }
        Err(err) => AuthResult::fail_with_stage(
            0,
            format!("网关网络请求失败: {}", err),
            "E3-01",
            "无法连接认证服务器，局域网连接可能不稳定。".to_string(),
        ),
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
fn ensure_winsock_initialized() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        use windows_sys::Win32::Networking::WinSock::{WSAStartup, WSADATA};
        let mut wsa_data: WSADATA = std::mem::zeroed();
        WSAStartup(0x0202, &mut wsa_data);
    });
}

#[cfg(windows)]
fn connect_lan_bound_tcp(
    remote_ip: &str,
    port: u16,
    timeout_ms: u32,
) -> std::io::Result<std::net::TcpStream> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::Networking::WinSock::*;

    ensure_winsock_initialized();

    let local_lan_ip = crate::config::get_lan_adapter_ipv4();
    let remote_v4 = match remote_ip.parse::<std::net::Ipv4Addr>() {
        Ok(v4) => v4,
        Err(_) => {
            use std::net::ToSocketAddrs;
            let mut resolved = None;
            if let Ok(iter) = (remote_ip, port).to_socket_addrs() {
                for addr in iter {
                    if let std::net::SocketAddr::V4(v4_addr) = addr {
                        resolved = Some(*v4_addr.ip());
                        break;
                    }
                }
            }
            resolved.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("无法解析网关主机名: {}", remote_ip),
                )
            })?
        }
    };

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

        // 1. 设置为非阻塞模式以精准控制 connect 超时（杜绝 Windows 默认 21 秒 SYN 超时挂死主线程）
        let mut non_blocking: u32 = 1;
        ioctlsocket(sock, FIONBIO, &mut non_blocking);

        let ret = connect(
            sock,
            &remote_sin as *const _ as *const _,
            std::mem::size_of_val(&remote_sin) as i32,
        );

        if ret != 0 {
            let wsa_err = WSAGetLastError();
            // WSAEWOULDBLOCK = 10035
            if wsa_err == 10035 {
                let mut write_fds: FD_SET = std::mem::zeroed();
                write_fds.fd_count = 1;
                write_fds.fd_array[0] = sock;

                let mut except_fds: FD_SET = std::mem::zeroed();
                except_fds.fd_count = 1;
                except_fds.fd_array[0] = sock;

                let tv_sec = (timeout_ms / 1000) as i32;
                let tv_usec = ((timeout_ms % 1000) * 1000) as i32;
                let tv = TIMEVAL {
                    tv_sec,
                    tv_usec,
                };

                let sel = select(
                    0,
                    std::ptr::null_mut(),
                    &mut write_fds,
                    &mut except_fds,
                    &tv,
                );

                if sel <= 0 {
                    closesocket(sock);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("连接网关 ({}:{}) 超时 ({}ms)", remote_ip, port, timeout_ms),
                    ));
                }

                let mut so_err: i32 = 0;
                let mut so_err_len = std::mem::size_of_val(&so_err) as i32;
                getsockopt(
                    sock,
                    SOL_SOCKET,
                    SO_ERROR,
                    &mut so_err as *mut _ as *mut _,
                    &mut so_err_len,
                );
                if so_err != 0 {
                    closesocket(sock);
                    return Err(std::io::Error::from_raw_os_error(so_err));
                }
            } else {
                closesocket(sock);
                return Err(std::io::Error::from_raw_os_error(wsa_err));
            }
        }

        // 2. 恢复为阻塞模式，以便后续标准读写及超时控制生效
        let mut blocking: u32 = 0;
        ioctlsocket(sock, FIONBIO, &mut blocking);

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
        params.insert("userId".to_string(), "2023000001".to_string());
        // No password
        let res = execute_ruijie_sam_auth("http://10.10.200.102/eportal/InterFace.do?method=login", &params, &BTreeMap::new());
        assert!(!res.success);
        assert!(res.message.contains("未配置校园网密码"));
    }

    #[test]
    fn test_ruijie_sam_query_string_cleaning() {
        let mut qs = "?wlanuserip=1.2.3.4&wlanacname=test";
        if let Some(pos) = qs.find('?') {
            qs = &qs[pos + 1..];
        }
        assert_eq!(qs, "wlanuserip=1.2.3.4&wlanacname=test");

        let enc = encode_ruijie_query_string(qs);
        assert_eq!(enc, "wlanuserip%253D1.2.3.4%2526wlanacname%253Dtest");
        assert!(enc.contains("%253D"));
        assert!(enc.contains("%2526"));
    }

    #[test]
    fn test_ruijie_rsa_encryption() {
        let plain = "78c0c92a0ae8d1ddbc96ebb09223ddbc>516382";
        let exp = "10001";
        let modulus = "94dd2a8675fb779e6b9f7103698634cd400f27a154afa67af6166a43fc26417222a79506d34cacc7641946abda1785b7acf9910ad6a0978c91ec84d40b71d2891379af19ffb333e7517e390bd26ac312fe940c340466b4a5d4af1d65c3b5944078f96a1a51a5a53e4bc302818b7c9f63c4a1b07bd7d874cef1c3d4b2f5eb7871";
        let encrypted = rsa_encrypt_ruijie(plain, exp, modulus).expect("encrypt");
        assert_eq!(
            encrypted,
            "7ac89527f83a1bd9ca95b6f9641987957a97f6c61f5a72d18181b9d3218a55d805699959af98fb9f1e4b7bf74aaa861f75917a4d84365db9caabb92fd6508666ad7f7692769c1d6341a4bb493ce71eb4538e398c33c9e7437afd58b3cb0bf8951020ce245499dc0725c242e77b9e59e6111970dbf3804126c6c1fb438dd43e55"
        );
    }
}
