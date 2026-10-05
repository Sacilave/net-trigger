pub mod browser;
pub mod http_client;
pub mod ruijie;

use crate::config::{Config, MacroContext};
pub use browser::{open_browser_portal, BrowserError};
pub use http_client::{execute_http_auth, AuthResult};
pub use ruijie::{get_fresh_ruijie_portal_url, get_fresh_ruijie_query_string};

/// 认证动作执行调度器
pub struct AuthExecutor;

impl AuthExecutor {
    /// 执行认证流水线
    ///
    /// 自动根据配置的 mode ("browser" / "http") 派发执行：
    /// - "browser" (自动打开网页登录·有弹窗)：无需用户名密码，掉线时唤起浏览器由已保存密码自动登录；
    /// - "http" (后台静默登录·无弹窗)：根据配置后台静默发包，打游戏不弹窗不切屏。
    pub fn execute(
        config: &Config,
        detected_portal_url: Option<&str>,
        allow_browser_open: bool,
    ) -> AuthResult {
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
                if let Some(fresh_url) = ruijie::get_fresh_ruijie_portal_url() {
                    fresh_url
                } else {
                    "http://123.123.123.123/".to_string()
                }
            } else {
                target_url.to_string()
            };

            // 若正处于防重弹冷却期内，跳过重复拉起外部浏览器动作，保持静默等待
            if !allow_browser_open {
                return AuthResult::fail_with_stage(
                    0,
                    "已在此前打开登录网页，等待网页端登录完成".to_string(),
                    "E5-BROWSER-WAITING",
                    "请在已打开的网页中完成登录。如未打开可右键托盘点击【打开认证网页】。".to_string(),
                );
            }

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
            let params = config.get_expanded_params_with_ctx(&ctx);

            // 派发认证驱动：若是锐捷 RG-SAM+ / eportal 专精处理，否则使用通用 HTTP 客户端
            let result = if action_url.contains("InterFace.do") || action_url.contains("eportal") {
                ruijie::execute_ruijie_sam_auth(
                    &action_url,
                    &params,
                    &config.auth.http.headers,
                    detected_portal_url,
                )
            } else {
                execute_http_auth(&config.auth.http, &action_url, &params)
            };

            // 若 HTTP 模拟认证失败且显式允许了浏览器兜底
            if !result.success && config.general.allow_browser_fallback {
                let fallback_url = detected_portal_url
                    .filter(|u| !u.trim().is_empty() && !u.contains("success.jsp"))
                    .unwrap_or(if !config.auth.portal_url.contains("success.jsp") && !config.auth.portal_url.is_empty() {
                        &config.auth.portal_url
                    } else {
                        "http://123.123.123.123/"
                    });

                // 若正处于防重弹冷却期内，跳过重复拉起外部浏览器动作，保持静默等待
                if !allow_browser_open {
                    return AuthResult::fail_with_stage(
                        0,
                        "静默登录未成功，已在此前唤起浏览器，等待网页端登录完成".to_string(),
                        "E5-FALLBACK-WAITING",
                        "请在已打开的校园网页面中完成登录，或右键托盘点击【打开认证网页】。".to_string(),
                    );
                }

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

        let res = AuthExecutor::execute(&cfg, None, true);
        assert!(!res.success);
        assert!(res.message.contains("未配置"));
    }

    #[test]
    fn test_auth_executor_browser_mode_success_jsp_sanitized() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=123".to_string();

        let res = AuthExecutor::execute(&cfg, None, true);
        assert!(res.success);
    }

    #[test]
    fn test_auth_executor_browser_mode_prefers_detected_url() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=123".to_string();

        let res = AuthExecutor::execute(&cfg, Some("http://10.10.200.102/eportal/index.jsp?wlanuserip=10.0.0.1"), true);
        assert!(res.success);
    }

    #[test]
    fn test_auth_executor_browser_mode_respects_cooldown_gate() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.auth.portal_url = "http://10.10.200.102/eportal/index.jsp".to_string();

        // 当 allow_browser_open = false 时，严禁拉起浏览器，必须返回等待状态
        let res = AuthExecutor::execute(&cfg, None, false);
        assert!(!res.success);
        assert_eq!(res.stage_code, "E5-BROWSER-WAITING");
    }
}
