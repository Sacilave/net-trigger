pub mod browser;
pub mod http_client;

use crate::config::{Config, MacroContext};
pub use browser::{open_browser_portal, BrowserError};
pub use http_client::{execute_http_auth, AuthResult};

/// 认证动作执行调度器
pub struct AuthExecutor;

impl AuthExecutor {
    /// 执行认证流水线
    ///
    /// 自动根据配置的 mode ("browser" / "http") 派发执行：
    /// - "browser" (自动打开网页登录·有弹窗)：无需用户名密码，掉线时唤起浏览器由已保存密码自动登录；
    /// - "http" (后台静默登录·无弹窗)：根据配置后台静默发包，打游戏不弹窗不切屏。
    pub fn execute(config: &Config, detected_portal_url: Option<&str>) -> AuthResult {
        let mode = config.auth.mode.to_lowercase();

        if mode == "browser" {
            let configured_url = config.auth.portal_url.trim();

            let target_url = if let Some(url) = detected_portal_url {
                if !url.trim().is_empty() {
                    url.trim()
                } else if !configured_url.is_empty() {
                    configured_url
                } else {
                    ""
                }
            } else if !configured_url.is_empty() {
                configured_url
            } else {
                ""
            };

            if target_url.is_empty() {
                return AuthResult::fail_with_stage(
                    0,
                    "未配置登录网页网址 (portal_url)".to_string(),
                    "E4-01",
                    "请右键托盘图标【设置】，填入校园网登录网页的 URL 地址".to_string(),
                );
            }

            // 防踩坑自愈：
            // 1. 若网址包含 success.jsp（用户误填了登录成功页），直接访问会导致校园网报“原ip与当前用户不一致”
            // 2. 若网址包含静态 wlanuserip= 但属于之前旧会话的参数，直接访问同样会报“原ip与当前用户不一致”
            // 3. 若 target_url 为未带会话参数的纯根路径（如 http://10.10.200.102/）
            // 此时自动通过局域网网卡探测或向 123.123.123.123 触发网关带当前动态真实 IP 的 302 重定向
            let final_url = if target_url.contains("success.jsp")
                || (target_url.contains("wlanuserip=") && detected_portal_url.is_none())
                || target_url == "http://10.10.200.102/"
                || target_url == "http://10.10.200.102"
            {
                if let Some(fresh_url) = http_client::get_fresh_ruijie_portal_url() {
                    fresh_url
                } else {
                    "http://123.123.123.123/".to_string()
                }
            } else {
                target_url.to_string()
            };

            match open_browser_portal(&final_url, true, true) {
                Ok(_) => AuthResult::ok(200, "已唤起浏览器登录页面，由浏览器自动填充密码登录".to_string()),
                Err(err) => AuthResult::fail_with_stage(
                    0,
                    format!("唤起浏览器失败: {}", err),
                    "E5-01",
                    "未能打开默认浏览器，请检查 Windows 默认应用设置".to_string(),
                ),
            }
        } else {
            // mode == "http" (静默模拟登录)
            let ctx = MacroContext::build(config);
            let action_url = config.get_expanded_action_url(&ctx);
            let mut params = config.get_expanded_params_with_ctx(&ctx);

            // 锐捷 RG-SAM+ / eportal 校园网智能参数自动补全：
            // 当 action_url 包含 InterFace.do 或 eportal 时，始终优先采用当前网络会话的动态 queryString：
            if action_url.contains("InterFace.do") || action_url.contains("eportal") {
                let mut dynamic_qs = None;

                // 优先级 1: 本次探测即时捕获到的认证重定向 URL (包含最新的动态 IP 与会话参数)
                if let Some(source_url) = detected_portal_url {
                    if let Some(q_pos) = source_url.find('?') {
                        let qs = &source_url[q_pos + 1..];
                        if !qs.is_empty() {
                            dynamic_qs = Some(qs.to_string());
                        }
                    }
                }

                // 优先级 2: 从局域网物理网卡向外网发起瞬时探测，抓取 AC 返回的最新 302 重定向
                if dynamic_qs.is_none() {
                    dynamic_qs = http_client::get_fresh_ruijie_query_string();
                }

                // 若成功抓取到最新的动态 queryString，强制覆盖（杜绝使用旧 IP 缓存造成"原ip与当前用户不一致"）
                if let Some(qs) = dynamic_qs {
                    params.insert("queryString".to_string(), qs);
                } else {
                    // 仅当动态探测完全无响应时，才降级使用配置中的 queryString 作为兜底
                    let has_valid_qs = params
                        .get("queryString")
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false);

                    if !has_valid_qs {
                        let cur_portal = &config.auth.portal_url;
                        if let Some(q_pos) = cur_portal.find('?') {
                            let qs = &cur_portal[q_pos + 1..];
                            if !qs.is_empty() {
                                params.insert("queryString".to_string(), qs.to_string());
                            }
                        }
                    }
                }
                if !params.contains_key("passwordEncrypt") {
                    params.insert("passwordEncrypt".to_string(), "false".to_string());
                }
                if !params.contains_key("service") {
                    params.insert("service".to_string(), "".to_string());
                }
                // 兼容 userId 别名
                if let Some(username_val) = params.get("username").cloned() {
                    params.entry("userId".to_string()).or_insert(username_val);
                }
            }

            let result = execute_http_auth(&config.auth.http, &action_url, &params);

            // 若 HTTP 模拟认证失败且显式允许了浏览器兜底
            if !result.success && config.general.allow_browser_fallback {
                let fallback_url = detected_portal_url
                    .filter(|u| !u.trim().is_empty() && !u.contains("success.jsp"))
                    .unwrap_or(if !config.auth.portal_url.contains("success.jsp") && !config.auth.portal_url.is_empty() {
                        &config.auth.portal_url
                    } else {
                        "http://123.123.123.123/"
                    });
                if open_browser_portal(fallback_url, true, false).is_ok() {
                    return AuthResult::fail_with_stage(
                        0,
                        "静默登录未成功，已自动唤起浏览器登录页面".to_string(),
                        "E5-FALLBACK-BROWSER",
                        "已为您打开校园网登录页面，请在网页中完成登录。".to_string(),
                    );
                }
            }

            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_executor_browser_mode_empty_url() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "".to_string();

        let res = AuthExecutor::execute(&cfg, None);
        assert!(!res.success);
        assert!(res.message.contains("未配置"));
    }

    #[test]
    fn test_auth_executor_browser_mode_success_jsp_sanitized() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=123".to_string();

        let res = AuthExecutor::execute(&cfg, None);
        // Should succeed in invoking browser with fallback rather than failing
        assert!(res.success);
    }

    #[test]
    fn test_auth_executor_browser_mode_prefers_detected_url() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=123".to_string();

        let res = AuthExecutor::execute(&cfg, Some("http://10.10.200.102/eportal/index.jsp?wlanuserip=10.0.0.1"));
        assert!(res.success);
    }
}
