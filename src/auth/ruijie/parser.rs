//! 锐捷 RG-SAM+ / ePortal 响应解析与故障诊断分类器

use crate::auth::AuthResult;

/// 快速提取非规范 JSON 字符串中的字段值
pub fn extract_json_field(json: &str, field_name: &str) -> Option<String> {
    let key_patterns = [
        format!("\"{}\":\"", field_name),
        format!("\"{}\": \"", field_name),
        format!("'{}':'", field_name),
        format!("'{}': '", field_name),
    ];

    for pattern in &key_patterns {
        if let Some(pos) = json.find(pattern) {
            let remainder = &json[pos + pattern.len()..];
            let end_char = if pattern.ends_with('\'') { '\'' } else { '"' };
            if let Some(end_pos) = remainder.find(end_char) {
                return Some(remainder[..end_pos].to_string());
            }
        }
    }
    None
}

/// 解析锐捷 SAM+ 服务端响应并生成结构化认证结果
pub fn parse_ruijie_response(resp_str: &str) -> AuthResult {
    if resp_str.contains("\"result\":\"success\"")
        || resp_str.contains("\"result\": \"success\"")
        || resp_str.contains("redirectortosuccess.jsp")
        || resp_str.contains("success.jsp")
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

/// 锐捷 SAM+ 错误信息细粒度归类与排障建议映射
pub fn classify_ruijie_error(msg: &str) -> (&'static str, String) {
    let lower = msg.to_lowercase();

    if msg.contains("密码错误") || msg.contains("口令错误") || lower.contains("password error") || lower.contains("ldap auth error") {
        (
            "E4-02",
            "校园网密码错误，请右键托盘图标【设置】核对并重新输入正确的校园网密码。".to_string(),
        )
    } else if msg.contains("用户不存在") || msg.contains("账号不存在") || lower.contains("user not exist") || lower.contains("user is not found") {
        (
            "E4-01",
            "校园网学号/账号不存在，请检查配置中的学号是否正确输入。".to_string(),
        )
    } else if msg.contains("欠费") || msg.contains("停机") || msg.contains("余额不足") || lower.contains("arrearage") {
        (
            "E4-03",
            "校园网账户已欠费停机，请前往学校网上缴费平台充值后重连。".to_string(),
        )
    } else if msg.contains("原ip") || msg.contains("设备未注册") || msg.contains("queryString") || msg.contains("参数错误") {
        (
            "E2-02",
            "Wi-Fi 会话参数或 IP 已过期变更，NetTrigger 正在自动刷新最新链路参数...".to_string(),
        )
    } else if msg.contains("在线人数超限") || msg.contains("设备数已满") || lower.contains("max user") || lower.contains("limit") {
        (
            "E4-04",
            "已达到当前校园网允许的最大同时在线设备数量上限，请在已连设备上下线一台后再试。".to_string(),
        )
    } else {
        (
            "E4-05",
            format!("校园网网关提示: {}。若反复出现可尝试托盘菜单【打开认证网页】手动登录。", msg),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_field() {
        let json = "{\"result\":\"fail\",\"message\":\"密码错误\"}";
        assert_eq!(extract_json_field(json, "result"), Some("fail".to_string()));
        assert_eq!(extract_json_field(json, "message"), Some("密码错误".to_string()));
        assert_eq!(extract_json_field(json, "other"), None);
    }

    #[test]
    fn test_parse_ruijie_response_success() {
        let resp_json = "{\"result\":\"success\",\"message\":\"\"}";
        let res = parse_ruijie_response(resp_json);
        assert!(res.success);
        assert_eq!(res.status_code, 200);

        let resp_redirect = "HTTP/1.1 302 Found\r\nLocation: http://10.10.200.102/eportal/redirectortosuccess.jsp\r\n";
        let res2 = parse_ruijie_response(resp_redirect);
        assert!(res2.success);
    }

    #[test]
    fn test_parse_ruijie_response_fail() {
        let resp_json = "{\"result\":\"fail\",\"message\":\"密码错误\"}";
        let res = parse_ruijie_response(resp_json);
        assert!(!res.success);
        assert_eq!(res.stage_code, "E4-02");
        assert!(res.message.contains("密码错误"));
    }
}

