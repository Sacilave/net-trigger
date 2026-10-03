//! 路径与文件系统工具模块
//!
//! 严格遵守 AGENTS.md 准则：所有路径必须基于当前可执行文件绝对路径锚定，
//! 严禁使用相对路径，免疫开机自启动与快捷方式引发的工作目录偏移问题。

use std::path::{Path, PathBuf};

/// 获取当前可执行程序所在的绝对父目录。
///
/// 若获取失败（极罕见环境），安全降级为当前目录，杜绝 panic。
pub fn get_app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 获取配置文件 (config.toml) 的绝对物理路径。
pub fn get_config_path() -> PathBuf {
    get_app_dir().join("config.toml")
}

/// 安全解析相对于应用程序根目录的相对路径，转换为绝对路径。
pub fn resolve_app_path<P: AsRef<Path>>(relative: P) -> PathBuf {
    get_app_dir().join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_app_dir_is_not_empty() {
        let dir = get_app_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn test_get_config_path_ends_with_config_toml() {
        let p = get_config_path();
        assert!(p.ends_with("config.toml"));
    }
}
