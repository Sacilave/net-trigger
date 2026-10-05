//! 锐捷 RG-SAM+ / ePortal 校园网认证驱动核心编排
//!
//! 核心设计：
//! 1. 严格对齐 Letki / 权威开源标准，精确保障 Referer 与双重转义报文合规；
//! 2. 动静结合 QueryString 嗅探，彻底规避 IP 漂移与会话过期；
//! 3. 智能 RSA 模幂加密自适应（协商开启 / 明文降级）；
//! 4. 链路自愈重试：遭遇“原ip不一致”时自动刷新会话并立即二次重试。

pub mod crypto;
pub mod parser;
pub mod query;

pub use crypto::rsa_encrypt_ruijie;
pub use parser::{classify_ruijie_error, parse_ruijie_response};
pub use query::{
    encode_ruijie_query_string, extract_query_param, extract_qs_from_url,
    get_fresh_ruijie_portal_url, get_fresh_ruijie_query_string,
};

use crate::auth::http_client::urlencoding_encode;
use crate::auth::AuthResult;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

/// 解析 URL 中的 host, port, path
pub fn parse_url_components(url: &str) -> (String, u16, String) {
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

/// 针对锐捷 RG-SAM+ / eportal 校园网的专属后台静默登录执行器
pub fn execute_ruijie_sam_auth(
    action_url: &str,
    params: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
    detected_portal_url: Option<&str>,
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

    let (host, port, _path) = parse_url_components(action_url);

    // 1. 多源动态获取当前最新的会话 queryString
    let mut query_string = String::new();

    // 优先级 1: 探针实时截获的 302 重定向 URL
    if let Some(detected) = detected_portal_url {
        if let Some(qs) = extract_qs_from_url(detected) {
            query_string = qs;
        }
    }

    // 优先级 2: 双轨实时嗅探（直连网关内网端点 + 劫持探针）
    if query_string.is_empty() {
        if let Some(fresh_qs) = query::get_fresh_ruijie_query_string_for_host(&host, port) {
            query_string = fresh_qs;
        }
    }

    // 优先级 3: 配置或传入参数中已有的 queryString
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

    // 优先级 4: 从 action_url 自身尾部提取
    if query_string.is_empty() {
        if let Some(qs) = extract_qs_from_url(action_url) {
            query_string = qs;
        }
    }

    if query_string.is_empty() {
        return AuthResult::fail_with_stage(
            400,
            "无法获取校园网认证参数 (queryString 为空)".to_string(),
            "E2-02",
            "未能从 Wi-Fi 网络捕获到 wlanuserip 会话参数。请确认已连接校园网 Wi-Fi。".to_string(),
        );
    }

    // 2. 密码加密自适应协商 (探测网关是否开启 RSA 密码加密)
    let (pwd_to_send, encrypt_flag) = if let Some((exp, modulus)) = crypto::fetch_ruijie_page_info(&host, port, &query_string) {
        let mac_str = extract_query_param(&query_string, "mac").unwrap_or_else(|| "111111111".to_string());
        let mac_str = if mac_str.is_empty() { "111111111".to_string() } else { mac_str };
        let plain = format!("{}>{}", password, mac_str);
        let reversed: String = plain.chars().rev().collect();
        if let Some(encrypted) = crypto::rsa_encrypt_ruijie(&reversed, &exp, &modulus) {
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

    // 3. 构建合规报文
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

    // 4. 构建强制请求头（注入关键 Referer）
    let agent = ureq::builder()
        .redirects(0)
        .timeout_connect(Duration::from_millis(3000))
        .timeout_read(Duration::from_millis(4000))
        .build();

    let mut req = agent.post(action_url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    req = req.set("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8");
    req = req.set("Referer", &format!("http://{}:{}/eportal/index.jsp?{}", host, port, query_string));
    req = req.set("Origin", &format!("http://{}:{}", host, port));
    req = req.set("Accept", "application/json, text/javascript, */*; q=0.01");
    req = req.set("X-Requested-With", "XMLHttpRequest");
    req = req.set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");

    let result = match req.send_string(&body) {
        Ok(response) => {
            let mut buf = [0u8; 8192];
            let mut reader = response.into_reader().take(8192);
            let n = reader.read(&mut buf).unwrap_or(0);
            let resp_str = String::from_utf8_lossy(&buf[..n]);
            parser::parse_ruijie_response(&resp_str)
        }
        Err(err) => AuthResult::fail_with_stage(
            0,
            format!("网关网络请求失败: {}", err),
            "E3-01",
            format!("无法与校园网认证服务器 ({}:{}) 建立连接：{}。请检查 Wi-Fi 是否连接正常。", host, port, err),
        ),
    };

    if result.success {
        return result;
    }

    // 5. 自愈重试：若网关报"原ip与当前用户不一致"或"参数错误"，说明之前捕获的会话参数过期，刷新后重试一次
    if result.message.contains("原ip")
        || result.message.contains("设备未注册")
        || result.message.contains("queryString")
        || result.message.contains("参数")
        || result.message.contains("为空")
    {
        if let Some(retry_qs) = query::get_fresh_ruijie_query_string_for_host(&host, port) {
            if retry_qs != query_string {
                let mut retry_params = params.clone();
                retry_params.insert("queryString".to_string(), retry_qs);
                return execute_ruijie_sam_auth(action_url, &retry_params, headers, None);
            }
        }
    }

    result
}
