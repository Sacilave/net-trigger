//! 内存与工作集管理工具
//!
//! 在冷启动就绪或大型临时任务（如临时 Web 服务）退出后，
//! 安全触发单次 Windows 工作集回收，释放启动期冷数据页面回系统备用列表。

/// 安全回收当前进程未使用的内存工作集（Working Set Trim）
///
/// 核心准则：仅在冷启动完成或大型临时资源销毁后执行，严禁在循环中高频调用。
pub fn trim_working_set() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_working_set_does_not_crash() {
        trim_working_set();
    }
}
