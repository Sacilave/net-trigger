//! 浏览器唤起模块 (Fallback 兜底)
//!
//! 核心设计：
//! 1. 严格受到 `allow_browser_fallback` 与 `silent_mode` 门控保护；
//! 2. 打游戏、全屏观影或默认静默模式下，绝对禁止强行拉起外部浏览器夺焦；
//! 3. 仅在明确允许或用户从托盘菜单点击时，通过 Win32 `ShellExecuteW` 原生唤起。

use std::fmt;

#[derive(Debug)]
pub enum BrowserError {
    BlockedBySilentMode,
    Win32(String),
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrowserError::BlockedBySilentMode => write!(
                f,
                "防夺焦保护已生效：静默/电竞模式下禁止自动唤起外部浏览器切屏"
            ),
            BrowserError::Win32(msg) => write!(f, "调用系统默认浏览器失败: {}", msg),
        }
    }
}

impl std::error::Error for BrowserError {}

/// 安全唤起默认系统浏览器打开指定 URL
///
/// - `url`: 登录门户地址
/// - `allow_fallback`: 是否允许自动唤起（由配置文件 `allow_browser_fallback` 决定）
/// - `force_user_action`: 是否为用户主动操作（如右键托盘点击“打开认证页面”），此时绕过静默限制
pub fn open_browser_portal(
    url: &str,
    allow_fallback: bool,
    force_user_action: bool,
) -> Result<(), BrowserError> {
    if !force_user_action && !allow_fallback {
        return Err(BrowserError::BlockedBySilentMode);
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        // 转为 UTF-16 宽字符并添加 \0 结尾
        let wide_operation: Vec<u16> = "open\0".encode_utf16().collect();
        let wide_url: Vec<u16> = format!("{}\0", url).encode_utf16().collect();

        unsafe {
            let res = ShellExecuteW(
                std::ptr::null_mut(),
                wide_operation.as_ptr(),
                wide_url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL as i32,
            );

            // ShellExecute 返回值大于 32 表示执行成功
            if (res as isize) > 32 {
                Ok(())
            } else {
                Err(BrowserError::Win32(format!(
                    "ShellExecuteW 返回错误代码: {}",
                    res as isize
                )))
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = url;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silent_mode_blocks_browser_popup() {
        // allow_fallback = false, force_user_action = false 必须被拦截
        let res = open_browser_portal("http://example.com", false, false);
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), BrowserError::BlockedBySilentMode));
    }
}
