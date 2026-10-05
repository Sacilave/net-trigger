//! 锐捷 RG-SAM+ / ePortal 动态参数嗅探与安全编码
//!
//! 核心设计：
//! 1. 规避裸 IP 与 DNS 挂起：优先直连网关内网端点 (零 DNS 依赖，10ms 极速响应)；
//! 2. 备用探针劫持嗅探：捕获 AC 硬件下发的 302 Location；
//! 3. 精确替换编码：严格遵守锐捷浏览器端两次 encodeURIComponent 等价替换原则 (& -> %2526, = -> %253D)；
//! 4. 彻底消除多网卡冲突与系统代理死锁。

use std::time::Duration;

/// 锐捷 SAM+ queryString 专用编码：仅替换 & 为 %2526、= 为 %253D
pub fn encode_ruijie_query_string(raw_qs: &str) -> String {
    raw_qs.replace('&', "%2526").replace('=', "%253D")
}

/// 从 URL 查询字符串中提取指定参数值
pub fn extract_query_param(qs: &str, param_name: &str) -> Option<String> {
    let prefix = format!("{}=", param_name);
    for part in qs.split('&') {
        if let Some(val) = part.strip_prefix(&prefix) {
            return Some(val.to_string());
        }
    }
    None
}

/// 从任意 URL 中提取查询字符串 (不含开头的 '?')
pub fn extract_qs_from_url(url: &str) -> Option<String> {
    if let Some(pos) = url.find('?') {
        let qs = &url[pos + 1..];
        if !qs.trim().is_empty() {
            return Some(qs.trim().to_string());
        }
    }
    None
}

fn check_target_for_redirect(agent: &ureq::Agent, target: &str) -> Option<String> {
    let res = agent.get(target).call();
    match res {
        Ok(resp) => {
            if let Some(loc) = resp.header("Location").or_else(|| resp.header("location")) {
                let loc_trim = loc.trim();
                if loc_trim.contains('?') && (loc_trim.contains("wlanuserip=") || loc_trim.contains("eportal") || loc_trim.contains("index.jsp")) {
                    return Some(loc_trim.to_string());
                }
            }
            let mut buf = [0u8; 4096];
            let mut reader = resp.into_reader().take(4096);
            use std::io::Read;
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);
            crate::probe::extract_redirect_url_from_html(&snippet)
        }
        Err(ureq::Error::Status(code, resp)) => {
            if (300..=399).contains(&code) {
                if let Some(loc) = resp.header("Location").or_else(|| resp.header("location")) {
                    let loc_trim = loc.trim();
                    if loc_trim.contains('?') && (loc_trim.contains("wlanuserip=") || loc_trim.contains("eportal") || loc_trim.contains("index.jsp")) {
                        return Some(loc_trim.to_string());
                    }
                }
            }
            let mut buf = [0u8; 4096];
            let mut reader = resp.into_reader().take(4096);
            use std::io::Read;
            let n = reader.read(&mut buf).unwrap_or(0);
            let snippet = String::from_utf8_lossy(&buf[..n]);
            crate::probe::extract_redirect_url_from_html(&snippet)
        }
        Err(_) => None,
    }
}

/// 嗅探当前网络会话的真实认证重定向地址
///
/// 架构设计（双轨嗅探）：
/// 1. 轨道 1（零 DNS 依赖·内网直连）：直连网关端点 `http://{host}:{port}/eportal/index.jsp`。
///    校园网未认证时，网关检测到请求缺失 wlanuserip 会自动返回 302，带上当前分配的真实 IP 与参数。
/// 2. 轨道 2（备选探针拦截）：向常用探测端点发送请求，捕获 AC 劫持的 302。
pub fn fetch_ruijie_redirect_url(gateway_host: &str, gateway_port: u16) -> Option<String> {
    let agent = ureq::builder()
        .redirects(0) // 显式禁用重定向，捕获 302 响应
        .timeout_connect(Duration::from_millis(1500))
        .timeout_read(Duration::from_millis(2000))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build();

    // 轨道 1: 网关内网直达探测（0 DNS 延迟，杜绝外网 DNS 丢包）
    let clean_host = gateway_host.trim();
    if !clean_host.is_empty() && clean_host != "127.0.0.1" && clean_host != "localhost" {
        let direct_targets = [
            format!("http://{}:{}/eportal/index.jsp", clean_host, gateway_port),
            format!("http://{}:{}/", clean_host, gateway_port),
        ];

        for target in &direct_targets {
            if let Some(url) = check_target_for_redirect(&agent, target) {
                return Some(url);
            }
        }
    }

    // 轨道 2: 标准外网端点劫持嗅探（AC 硬件必定劫持 80 端口外网流量并重定向到带完整参数的认证页）
    let probe_targets = [
        "http://123.123.123.123/",
        "http://www.msftconnecttest.com/connecttest.txt",
        "http://connect.rom.miui.com/generate_204",
        "http://captive.apple.com/hotspot-detect.html",
        "http://1.1.1.1/",
    ];

    for target in &probe_targets {
        if let Some(url) = check_target_for_redirect(&agent, target) {
            return Some(url);
        }
    }

    None
}

/// 针对指定网关实时获取最新的动态 queryString
pub fn get_fresh_ruijie_query_string_for_host(gateway_host: &str, gateway_port: u16) -> Option<String> {
    if let Some(url) = fetch_ruijie_redirect_url(gateway_host, gateway_port) {
        if let Some(qs) = extract_qs_from_url(&url) {
            return Some(qs);
        }
    }
    None
}

/// 针对指定网关实时获取最新的动态登录页面 URL
pub fn get_fresh_ruijie_portal_url_for_host(gateway_host: &str, gateway_port: u16) -> Option<String> {
    fetch_ruijie_redirect_url(gateway_host, gateway_port)
}

/// 兼容无参调用：智能多路嗅探最新动态 queryString
pub fn get_fresh_ruijie_query_string() -> Option<String> {
    for host in &["10.10.200.102", "10.255.253.2", "172.16.1.1"] {
        if let Some(qs) = get_fresh_ruijie_query_string_for_host(host, 80) {
            return Some(qs);
        }
    }
    if let Some(url) = fetch_ruijie_redirect_url("", 80) {
        if let Some(qs) = extract_qs_from_url(&url) {
            return Some(qs);
        }
    }
    None
}

/// 兼容无参调用：智能多路嗅探最新动态登录页面 URL
pub fn get_fresh_ruijie_portal_url() -> Option<String> {
    for host in &["10.10.200.102", "10.255.253.2", "172.16.1.1"] {
        if let Some(url) = fetch_ruijie_redirect_url(host, 80) {
            return Some(url);
        }
    }
    fetch_ruijie_redirect_url("", 80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_ruijie_query_string() {
        let raw = "wlanuserip=10.0.0.1&wlanacname=AC1&nasip=10.0.0.254";
        let encoded = encode_ruijie_query_string(raw);
        assert_eq!(encoded, "wlanuserip%253D10.0.0.1%2526wlanacname%253DAC1%2526nasip%253D10.0.0.254");
    }

    #[test]
    fn test_extract_query_param() {
        let qs = "wlanuserip=10.10.1.2&mac=AABBCCDDEEFF&t=wireless-v2";
        assert_eq!(extract_query_param(qs, "wlanuserip"), Some("10.10.1.2".to_string()));
        assert_eq!(extract_query_param(qs, "mac"), Some("AABBCCDDEEFF".to_string()));
        assert_eq!(extract_query_param(qs, "nonexistent"), None);
    }

    #[test]
    fn test_extract_qs_from_url() {
        let url = "http://10.10.200.102/eportal/index.jsp?wlanuserip=10.0.0.1&ssid=test";
        assert_eq!(extract_qs_from_url(url), Some("wlanuserip=10.0.0.1&ssid=test".to_string()));

        let url_no_qs = "http://10.10.200.102/eportal/index.jsp";
        assert_eq!(extract_qs_from_url(url_no_qs), None);
    }
}


