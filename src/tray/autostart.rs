//! Windows 开机自启管理模块 (基于当前用户注册表 HKCU\Run)
//!
//! 核心设计：
//! 1. 严格使用 `get_app_dir()` 锚定绝对路径，外层包裹转义双引号防止路径空格歧义；
//! 2. 仅写入当前用户注册表 `HKEY_CURRENT_USER`，完全免除管理员 UAC 弹窗提权扰民；
//! 3. 干净自愈，支持一键查询、开启与关闭。

use std::fmt;

const REG_RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const REG_APP_NAME: &str = "NetTrigger";

#[derive(Debug)]
pub enum AutoStartError {
    GetExePathFailed,
    Registry(String),
}

impl fmt::Display for AutoStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoStartError::GetExePathFailed => write!(f, "获取当前程序可执行文件路径失败"),
            AutoStartError::Registry(msg) => write!(f, "注册表操作失败: {}", msg),
        }
    }
}

impl std::error::Error for AutoStartError {}

/// 查询当前是否已开启开机自启
pub fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        };

        let subkey_wide: Vec<u16> = format!("{}\0", REG_RUN_SUBKEY).encode_utf16().collect();
        let value_name_wide: Vec<u16> = format!("{}\0", REG_APP_NAME).encode_utf16().collect();

        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let res = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_wide.as_ptr(),
                0,
                KEY_READ,
                &mut hkey,
            );

            if res != 0 {
                return false;
            }

            let mut data_len: u32 = 0;
            let query_res = RegQueryValueExW(
                hkey,
                value_name_wide.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut data_len,
            );

            RegCloseKey(hkey);
            query_res == 0 && data_len > 0
        }
    }

    #[cfg(not(windows))]
    {
        false
    }
}

/// 启用或关闭开机自启动
pub fn set_autostart(enable: bool) -> Result<(), AutoStartError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            KEY_SET_VALUE, REG_SZ,
        };

        let exe_path = std::env::current_exe().map_err(|_| AutoStartError::GetExePathFailed)?;
        // 关键安全规范：包裹双引号防止路径空格截断，并附带 --autostart 标识静默自启
        let quoted_cmd = format!("\"{}\" --autostart", exe_path.to_string_lossy());

        let subkey_wide: Vec<u16> = format!("{}\0", REG_RUN_SUBKEY).encode_utf16().collect();
        let value_name_wide: Vec<u16> = format!("{}\0", REG_APP_NAME).encode_utf16().collect();

        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            let open_res = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey_wide.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );

            if open_res != 0 {
                return Err(AutoStartError::Registry(format!(
                    "打开注册表项失败 (错误码: {})",
                    open_res
                )));
            }

            let op_res = if enable {
                let cmd_wide: Vec<u16> = format!("{}\0", quoted_cmd).encode_utf16().collect();
                let bytes_len = (cmd_wide.len() * std::mem::size_of::<u16>()) as u32;

                RegSetValueExW(
                    hkey,
                    value_name_wide.as_ptr(),
                    0,
                    REG_SZ,
                    cmd_wide.as_ptr() as *const u8,
                    bytes_len,
                )
            } else {
                let del_res = RegDeleteValueW(hkey, value_name_wide.as_ptr());
                // ERROR_FILE_NOT_FOUND (2) 时认为已经删除成功
                if del_res == 2 {
                    0
                } else {
                    del_res
                }
            };

            RegCloseKey(hkey);

            if op_res == 0 {
                Ok(())
            } else {
                Err(AutoStartError::Registry(format!(
                    "写入/删除注册表键值失败 (错误码: {})",
                    op_res
                )))
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = enable;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autostart_query_does_not_crash() {
        // 查询操作必须绝对稳定且不崩溃
        let _ = is_autostart_enabled();
    }
}
