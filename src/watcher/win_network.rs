//! Windows 原生网络状态事件驱动监听器
//!
//! 核心设计：
//! 1. 深度挂钩 Windows IP Helper API (`NotifyAddrChange`)；
//! 2. 线程基于操作系统内核事件句柄进入无损挂起 (`WaitForMultipleObjects`)，CPU 绝对 0.00%；
//! 3. 网卡重连/IP跳变瞬间（< 5ms）被动唤醒；
//! 4. 黄金链路稳定防抖窗口（Debounce Window），杜绝 DHCP 协商完成前的无效网络发包。

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkEvent {
    /// 操作系统报告物理链路、网卡或 IP 地址发生变动（已防抖）
    NetworkChanged,
    /// 监听器已正常退出
    WatcherStopped,
}

#[derive(Debug)]
pub enum WatcherError {
    Win32(String),
    ThreadSpawn(std::io::Error),
}

impl fmt::Display for WatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatcherError::Win32(msg) => write!(f, "Windows 网络监听原生错误: {}", msg),
            WatcherError::ThreadSpawn(err) => write!(f, "网络监听线程启动失败: {}", err),
        }
    }
}

impl std::error::Error for WatcherError {}

/// 网络事件监听器句柄，用于在程序退出时优雅终止监听线程
pub struct NetworkWatcherHandle {
    stop_signal: Arc<AtomicBool>,
    #[cfg(windows)]
    stop_event: windows_sys::Win32::Foundation::HANDLE,
    thread_handle: Option<JoinHandle<()>>,
}

// Windows HANDLE 在保证生命周期安全的前提下允许在线程间发送
unsafe impl Send for NetworkWatcherHandle {}
unsafe impl Sync for NetworkWatcherHandle {}

impl NetworkWatcherHandle {
    /// 优雅停止网络监听线程
    pub fn stop(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::SetEvent;
            if !self.stop_event.is_null() && self.stop_event != (-1isize as _) {
                unsafe {
                    SetEvent(self.stop_event);
                }
            }
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for NetworkWatcherHandle {
    fn drop(&mut self) {
        self.stop();
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            if !self.stop_event.is_null() && self.stop_event != (-1isize as _) {
                unsafe {
                    CloseHandle(self.stop_event);
                }
                self.stop_event = std::ptr::null_mut();
            }
        }
    }
}

/// 启动 Windows 原生网络状态监听器
///
/// - `debounce_ms`: 链路稳定防抖窗口（毫秒）
/// - `sender`: 接收网络变更通知的通道
pub fn start_network_watcher(
    debounce_ms: u64,
    sender: Sender<NetworkEvent>,
) -> Result<NetworkWatcherHandle, WatcherError> {
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_signal_clone = Arc::clone(&stop_signal);

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::NetworkManagement::IpHelper::{CancelIPChangeNotify, NotifyAddrChange};
        use windows_sys::Win32::System::IO::OVERLAPPED;
        use windows_sys::Win32::System::Threading::{
            CreateEventW, WaitForMultipleObjects, INFINITE,
        };

        unsafe {
            // 创建退出信号事件 (Manual Reset)
            let stop_event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if stop_event.is_null() {
                return Err(WatcherError::Win32("创建退出事件句柄失败".to_string()));
            }

            let stop_event_raw = stop_event as isize;

            let thread_handle = thread::Builder::new()
                .name("NetTrigger-Watcher".to_string())
                .spawn(move || {
                    let stop_event = stop_event_raw as HANDLE;
                    let net_event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                    if net_event.is_null() {
                        return;
                    }

                    let mut notify_handle: HANDLE = std::ptr::null_mut();
                    let mut overlapped: OVERLAPPED = std::mem::zeroed();
                    overlapped.hEvent = net_event;

                    let events = [stop_event, net_event];

                    loop {
                        if stop_signal_clone.load(Ordering::SeqCst) {
                            break;
                        }

                        // 异步注册网络状态变更通知
                        let res = NotifyAddrChange(&mut notify_handle, &overlapped);
                        // ERROR_IO_PENDING = 997, NO_ERROR = 0
                        if res != 0 && res != 997 {
                            // 极罕见错误发生时短暂休眠，避免死循环
                            thread::sleep(Duration::from_millis(500));
                            continue;
                        }

                        // 阻塞挂起等待内核唤醒 (CPU 0.00%)
                        let wait_result = WaitForMultipleObjects(2, events.as_ptr(), 0, INFINITE);

                        if wait_result == WAIT_OBJECT_0 {
                            // 收到停止信号
                            break;
                        } else if wait_result == WAIT_OBJECT_0 + 1 {
                            // 收到网络变更事件！进入黄金防抖窗口
                            // 物理网卡重新建立连接后，等待 DHCP 与本地路由完全稳定
                            thread::sleep(Duration::from_millis(debounce_ms.max(20)));

                            // 消费掉防抖期间可能堆积的重复信号
                            while WaitForMultipleObjects(1, &net_event, 0, 0) == WAIT_OBJECT_0 {
                                thread::sleep(Duration::from_millis(10));
                            }

                            // 发送防抖后的有效网络变动通知
                            if sender.send(NetworkEvent::NetworkChanged).is_err() {
                                break;
                            }
                        } else {
                            // 句柄或系统异常
                            break;
                        }
                    }

                    if !notify_handle.is_null() {
                        CancelIPChangeNotify(&overlapped);
                    }
                    CloseHandle(net_event);
                    let _ = sender.send(NetworkEvent::WatcherStopped);
                })
                .map_err(WatcherError::ThreadSpawn)?;

            Ok(NetworkWatcherHandle {
                stop_signal,
                stop_event,
                thread_handle: Some(thread_handle),
            })
        }
    }

    #[cfg(not(windows))]
    {
        let thread_handle = thread::Builder::new()
            .name("NetTrigger-Watcher-Mock".to_string())
            .spawn(move || {
                while !stop_signal_clone.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1000));
                }
                let _ = sender.send(NetworkEvent::WatcherStopped);
            })
            .map_err(WatcherError::ThreadSpawn)?;

        Ok(NetworkWatcherHandle {
            stop_signal,
            thread_handle: Some(thread_handle),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn test_network_watcher_lifecycle() {
        let (tx, rx) = channel();
        let handle_res = start_network_watcher(50, tx);
        assert!(handle_res.is_ok(), "网络监听器必须能够顺利启动");

        if let Ok(mut handle) = handle_res {
            // 短暂运行后优雅停止
            thread::sleep(Duration::from_millis(50));
            handle.stop();

            // 验证能够收到正常退出的 WatcherStopped
            let mut got_stopped = false;
            while let Ok(evt) = rx.recv_timeout(Duration::from_millis(300)) {
                if evt == NetworkEvent::WatcherStopped {
                    got_stopped = true;
                    break;
                }
            }
            assert!(got_stopped, "必须能够接收到停止事件");
        }
    }
}
