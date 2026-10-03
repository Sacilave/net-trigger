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
    /// 自动根据配置的 mode ("http" / "browser") 派发执行，
    /// 并执行动态宏参数替换与静默保护。
    pub fn execute(config: &Config) -> AuthResult {
        let mode = config.auth.mode.to_lowercase();

        if mode == "browser" {
            match open_browser_portal(
                &config.auth.portal_url,
                config.general.allow_browser_fallback,
                false,
            ) {
                Ok(_) => AuthResult::ok(200, "已通过默认浏览器唤起认证页面".to_string()),
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
    fn test_auth_executor_browser_mode() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        cfg.general.allow_browser_fallback = false;

        // 默认防夺焦阻止自动弹浏览器
        let res = AuthExecutor::execute(&cfg);
        assert!(!res.success);
        assert!(res.message.contains("防夺焦保护"));
    }
}
