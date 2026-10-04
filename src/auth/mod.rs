pub mod browser;
pub mod http_client;

use crate::config::{Config, MacroContext};
use std::time::Duration;
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
    pub fn execute(config: &Config) -> AuthResult {
        let mode = config.auth.mode.to_lowercase();

        if mode == "browser" {
            let portal_url = config.auth.portal_url.trim();
            if portal_url.is_empty() {
                return AuthResult::fail(0, "未配置登录网页网址 (portal_url)".to_string());
            }

            // 1. 先尝试一次超轻量静默 GET 访问 (很多网关有 MAC 快速免密或 Cookie 续期机制，只要访问一次即自动放行)
            let silent_touch = ureq::builder()
                .timeout_connect(Duration::from_millis(1500))
                .timeout_read(Duration::from_millis(2000))
                .redirects(2)
                .build()
                .get(portal_url)
                .call();

            if let Ok(resp) = silent_touch {
                if resp.status() == 200 || resp.status() == 204 {
                    // 静默访问成功触发了后端放行逻辑
                    return AuthResult::ok(resp.status(), "已静默访问登录网址触发认证".to_string());
                }
            }

            // 2. 若静默访问未直接放行，则唤起默认浏览器，由浏览器记住的密码自动登录
            match open_browser_portal(portal_url, true, true) {
                Ok(_) => AuthResult::ok(200, "已唤起浏览器登录页面，由浏览器自动填充密码登录".to_string()),
                Err(err) => AuthResult::fail(0, format!("唤起浏览器失败: {}", err)),
            }
        } else {
            // mode == "http" (静默模拟登录)
            let ctx = MacroContext::build(config);
            let action_url = config.get_expanded_action_url(&ctx);
            let params = config.get_expanded_params_with_ctx(&ctx);

            let result = execute_http_auth(&config.auth.http, &action_url, &params);

            // 若 HTTP 模拟认证失败且显式允许了浏览器兜底
            if !result.success && config.general.allow_browser_fallback {
                let _ = open_browser_portal(&config.auth.portal_url, true, false);
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

        let res = AuthExecutor::execute(&cfg);
        assert!(!res.success);
        assert!(res.message.contains("未配置"));
    }
}
