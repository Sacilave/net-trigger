#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! NetTrigger - 专为 Windows 平台打造的原生、超轻量、0 延迟网络事件自动化触发与认证守护引擎

pub mod auth;
pub mod config;
pub mod probe;
pub mod state;
pub mod tray;
pub mod utils;
pub mod watcher;
pub mod web_config;

use config::Config;
use state::StateMachine;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use tray::{SystemTrayManager, TrayAction};
use utils::fs::get_config_path;
use watcher::{start_network_watcher, NetworkEvent};

fn main() {
    // 0. 启动时自愈注册表自启动项：升级历史老版本可能未携带 --autostart 参数的旧键值
    tray::autostart::sync_and_heal_autostart_registry();

    // 判断是否为自动启动（开机自启参数、命令行静默参数或系统开机引导）
    let is_auto_launch = is_auto_launch_detected();

    // 1. 全局单实例互斥锁，防止多开
    #[cfg(windows)]
    let _single_instance_guard = match acquire_single_instance_mutex() {
        Ok(handle) => handle,
        Err(_) => {
            // 若已经在运行且用户手动双击唤起，弹出贴心提示告知已常驻托盘
            if !is_auto_launch {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_TOPMOST,
                    };
                    let sys_lang = config::detect_system_language();
                    let (title_str, msg_str) = match sys_lang {
                        config::Language::Zh => (
                            "NetTrigger 正在运行\0",
                            "NetTrigger 已经在后台运行中。\n\n程序已常驻任务栏右下角托盘（若未显示，请点击“^”展开查看）。\n鼠标左键或右键托盘图标均可打开【设置】。\0",
                        ),
                        _ => (
                            "NetTrigger is already running\0",
                            "NetTrigger is already running in the background.\n\nIt is minimized to the system tray (click '^' to show hidden icons if not visible).\nLeft-click or right-click the tray icon to open Settings.\0",
                        ),
                    };
                    let title: Vec<u16> = title_str.encode_utf16().collect();
                    let msg: Vec<u16> = msg_str.encode_utf16().collect();
                    MessageBoxW(
                        std::ptr::null_mut(),
                        msg.as_ptr(),
                        title.as_ptr(),
                        MB_OK | MB_ICONINFORMATION | MB_TOPMOST,
                    );
                }
            } else {
                eprintln!("NetTrigger 已经在运行中。");
            }
            return;
        }
    };

    println!("⚡ NetTrigger 正在启动...");

    // 2. 加载或原子自愈生成配置文件
    let (config, created) = match Config::load_or_create() {
        Ok(res) => res,
        Err(err) => {
            eprintln!("无法初始化配置文件: {}", err);
            return;
        }
    };

    let app_lang = config.general.effective_language();
    let app_name = config.general.app_name.clone();

    if created {
        match app_lang {
            config::Language::Zh => println!("✨ 首次启动，已在当前目录下自动生成带详细中文注释的 config.toml 配置文件。"),
            _ => println!("✨ First run: generated config.toml with detailed English comments."),
        }
    }

    // 3. 初始化极轻量系统托盘管理器 (若桌面 Shell 尚未完全就绪，允许在主事件循环中自愈挂载)
    let mut tray_mgr: Option<SystemTrayManager> = match SystemTrayManager::new(&app_name, config.general.silent_mode, app_lang) {
        Ok(mgr) => Some(mgr),
        Err(err) => {
            eprintln!("初始创建系统托盘暂未就绪（桌面环境就绪后将自动自愈挂接）: {}", err);
            None
        }
    };

    // 4. 若为手动双击启动（非开机自启/非静默模式），弹出精简易懂的消息提醒，告知用户程序已在托盘守护
    if !is_auto_launch {
        std::thread::spawn(move || {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    MessageBoxW, MB_ICONINFORMATION, MB_OK, MB_TOPMOST,
                };
                let (title_str, msg_str) = match app_lang {
                    config::Language::Zh => (
                        "NetTrigger 已启动\0",
                        "NetTrigger 已在后台启动并守护网络连接。\n\n程序已常驻任务栏右下角托盘（若未显示，请点击“^”展开查看）。\n鼠标左键或右键托盘图标均可打开【设置】。\0",
                    ),
                    _ => (
                        "NetTrigger Started\0",
                        "NetTrigger is running in the background and guarding your network connection.\n\nIt is minimized to the system tray (click '^' to show hidden icons if not visible).\nLeft-click or right-click the tray icon to open Settings.\0",
                    ),
                };
                let title: Vec<u16> = title_str.encode_utf16().collect();
                let msg: Vec<u16> = msg_str.encode_utf16().collect();
                MessageBoxW(
                    std::ptr::null_mut(),
                    msg.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONINFORMATION | MB_TOPMOST,
                );
            }
        });
    }

    // 5. 初始化核心有限状态机
    let mut fsm = StateMachine::new(config.clone());

    // 5. 启动 Windows 原生 IP Helper 网络状态被动监听器
    #[cfg(windows)]
    let main_thread_id = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    #[cfg(not(windows))]
    let main_thread_id = 0u32;

    let (net_tx, net_rx) = channel();
    let (config_tx, config_rx) = channel::<Config>();
    let debounce_ms = config.general.effective_debounce_ms();
    let mut watcher_handle = match start_network_watcher(debounce_ms, net_tx, main_thread_id) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("启动网络监听器失败 (回退至纯定时器模式): {}", err);
            // 依然保证程序运行，不崩溃
            watcher::win_network::start_network_watcher(150, channel().0, main_thread_id).unwrap_or_else(|_| {
                panic_free_fallback_handle(main_thread_id)
            })
        }
    };

    // 首次开机自检与即刻重连
    let initial_state = fsm.step_probe();
    if initial_state != state::NetworkState::Online {
        fsm.trigger_reconnect();
    }
    let snap = fsm.snapshot();
    if let Some(ref mut mgr) = tray_mgr {
        mgr.update_state(fsm.state(), snap.latency_ms, &app_name);
    }

    // 首次开机自检与托盘渲染就绪，安全回收冷启动期占用，常驻内存压缩至 ~1MB
    utils::mem::trim_working_set();

    // 6. 顶层无损事件主循环 (消息泵 + 内核事件 + 心跳定时器 + 配置热同步)
    let mut last_heartbeat = Instant::now();

    loop {
        // A. 动态计算本次循环最大休眠挂起时间（结合心跳保活与退避等待时间，彻底摆脱 100ms 盲轮询）
        let heartbeat_interval = Duration::from_secs(fsm.config().general.effective_heartbeat_interval_sec());
        let time_to_heartbeat = heartbeat_interval.saturating_sub(last_heartbeat.elapsed());
        let snap = fsm.snapshot();
        let wait_ms = if snap.backoff_remaining_sec > 0 {
            let backoff_ms = (snap.backoff_remaining_sec as u128 * 1000).min(u32::MAX as u128) as u32;
            (time_to_heartbeat.as_millis().min(u32::MAX as u128) as u32).min(backoff_ms)
        } else {
            time_to_heartbeat.as_millis().min(u32::MAX as u128) as u32
        };
        // 动态约束休眠等待区间在 20ms ~ 90s 之间
        let wait_ms = wait_ms.clamp(20, 90_000);

        // B. 处理 Win32 消息循环（驱动托盘交互，内核事件即时唤醒，CPU 严格 0.00%）
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
            };

            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // 托盘延迟自愈挂载（若开机自启初期 Explorer 尚未就绪）
        if tray_mgr.is_none() {
            let cur_cfg = fsm.config();
            if let Ok(mut mgr) = SystemTrayManager::new(&app_name, cur_cfg.general.silent_mode, cur_cfg.general.effective_language()) {
                let s = fsm.snapshot();
                mgr.update_state(fsm.state(), s.latency_ms, &app_name);
                tray_mgr = Some(mgr);
            }
        }

        // B. 即刻处理托盘右键菜单用户操作 (0ms 零延迟立即响应，绝不挂起等待！)
        let action = tray_mgr.as_ref().and_then(|mgr| mgr.poll_menu_event());
        if let Some(action) = action {
            match action {
                TrayAction::ManualCheck => {
                    let st = fsm.on_user_manual_trigger();
                    let s = fsm.snapshot();
                    if let Some(ref mut mgr) = tray_mgr {
                        mgr.update_state(st, s.latency_ms, &app_name);
                    }
                }
                TrayAction::OpenPortal => {
                    // 核心逻辑：优先使用状态机捕获到的网关真实认证重定向地址或净化后的入口，彻底杜绝 success.jsp 报“原ip与当前用户不一致”
                    let portal_url = fsm.get_effective_portal_url();
                    let _ = auth::browser::open_browser_portal(&portal_url, true, true);
                }
                TrayAction::OpenWebConfig => {
                    let latest_cfg = config::Config::load_or_create()
                        .map(|(c, _)| c)
                        .unwrap_or_else(|_| fsm.config().clone());
                    let _ = web_config::launch_ephemeral_web_config(latest_cfg, Some(config_tx.clone()), main_thread_id);
                }
                TrayAction::ToggleAutostart => {
                    let current_enabled = tray::autostart::is_autostart_enabled();
                    let next_state = !current_enabled;
                    let _ = tray::autostart::set_autostart(next_state);
                    if let Some(ref mgr) = tray_mgr {
                        mgr.sync_autostart_state(next_state);
                    }
                }
                TrayAction::ToggleSilentMode => {
                    let mut cfg = fsm.config().clone();
                    cfg.general.silent_mode = !cfg.general.silent_mode;
                    if let Ok(toml_str) = toml::to_string_pretty(&cfg) {
                        let _ = std::fs::write(get_config_path(), toml_str);
                    }
                    fsm.update_config(cfg);
                }
                TrayAction::SetLanguage(lang) => {
                    let mut cfg = fsm.config().clone();
                    cfg.general.language = lang;
                    if let Ok(toml_str) = toml::to_string_pretty(&cfg) {
                        let _ = std::fs::write(get_config_path(), toml_str);
                    }
                    if let Some(ref mut mgr) = tray_mgr {
                        mgr.set_language(lang, &app_name);
                    }
                    fsm.update_config(cfg);
                }
                TrayAction::Quit => {
                    println!("正在安全退出 NetTrigger...");
                    watcher_handle.stop();
                    break;
                }
            }
        }

        // C. 处理配置热保存通知（Web 配置页保存后即刻热同步状态机与托盘）
        while let Ok(new_cfg) = config_rx.try_recv() {
            println!("⚡ 收到配置热保存，立即热同步状态机与托盘...");
            let new_lang = new_cfg.general.effective_language();
            let new_app_name = new_cfg.general.app_name.clone();
            if let Some(ref mut mgr) = tray_mgr {
                mgr.set_language(new_lang, &new_app_name);
            }
            fsm.update_config(new_cfg);
            let st = fsm.on_network_changed();
            let s = fsm.snapshot();
            if let Some(ref mut mgr) = tray_mgr {
                mgr.update_state(st, s.latency_ms, &new_app_name);
            }
        }

        // D. 处理 Windows 底层网卡变动事件（近乎 0 延迟响应）
        while let Ok(net_evt) = net_rx.try_recv() {
            if net_evt == NetworkEvent::NetworkChanged {
                let st = fsm.on_network_changed();
                let s = fsm.snapshot();
                if let Some(ref mut mgr) = tray_mgr {
                    mgr.update_state(st, s.latency_ms, &app_name);
                }
            }
        }

        // D. 处理低频心跳定时器 (Heartbeat)
        let heartbeat_interval = Duration::from_secs(fsm.config().general.effective_heartbeat_interval_sec());
        if last_heartbeat.elapsed() >= heartbeat_interval {
            last_heartbeat = Instant::now();
            let st = fsm.on_heartbeat_tick();
            let s = fsm.snapshot();
            if let Some(ref mut mgr) = tray_mgr {
                mgr.update_state(st, s.latency_ms, &app_name);
            }
        }

        // E. 内核级挂起等待：所有就绪事件与操作处理完毕后，在循环最末端等待下一个事件信号或心跳超时
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait_ms, QS_ALLINPUT);
        }

        #[cfg(not(windows))]
        std::thread::sleep(Duration::from_millis(wait_ms as u64));
    }
}

/// Windows 命名互斥锁，确保全局单一实例
#[cfg(windows)]
fn acquire_single_instance_mutex() -> Result<windows_sys::Win32::Foundation::HANDLE, ()> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mutex_name: Vec<u16> = "Global\\NetTrigger_SingleInstance_Mutex\0"
        .encode_utf16()
        .collect();

    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr());
        // ERROR_ALREADY_EXISTS = 183
        if handle.is_null() || GetLastError() == 183 {
            return Err(());
        }
        Ok(handle)
    }
}

fn panic_free_fallback_handle(main_thread_id: u32) -> watcher::NetworkWatcherHandle {
    let (tx, _) = channel();
    watcher::win_network::start_network_watcher(150, tx, main_thread_id)
        .unwrap_or_else(|_| std::process::exit(1))
}

/// 智能判定当前运行是否为系统开机自启或后台静默启动
fn is_auto_launch_detected() -> bool {
    // 1. 显式命令行标志判定
    let has_flag = std::env::args().any(|arg| {
        arg == "--autostart"
            || arg == "--silent"
            || arg == "-s"
            || arg == "/silent"
            || arg == "--background"
            || arg == "-b"
    });
    if has_flag {
        return true;
    }

    // 2. Windows 平台开机自启多维辅助判定：
    // 若注册表中配置了开机自启动，则在冷启动或 Windows 快速启动 / 桌面会话登录 3 分钟以内，均视作开机自启
    #[cfg(windows)]
    {
        if tray::autostart::is_autostart_enabled() {
            // A. 冷启动：系统运行时间 3 分钟以内
            let tick_boot = unsafe {
                windows_sys::Win32::System::SystemInformation::GetTickCount64() < 180_000
            };
            if tick_boot {
                return true;
            }

            // B. 快速启动与用户登录判定：检查桌面 Shell (explorer.exe) 的建立时间
            if is_shell_or_session_startup() {
                return true;
            }
        }
    }

    false
}

#[cfg(windows)]
fn is_shell_or_session_startup() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

    unsafe {
        // 1. 如果桌面 Shell 窗口尚不存在，说明还在登录启动阶段
        let shell_hwnd = GetShellWindow();
        if shell_hwnd.is_null() {
            return true;
        }

        // 2. 获取 Shell (explorer.exe) 的进程创建时间
        let mut shell_pid = 0u32;
        GetWindowThreadProcessId(shell_hwnd, &mut shell_pid);
        if shell_pid == 0 || shell_pid == GetCurrentProcessId() {
            return false;
        }

        let h_shell = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, shell_pid);
        if h_shell.is_null() {
            return false;
        }

        let mut shell_creation = 0u64;
        let mut exit_time = 0u64;
        let mut kernel_time = 0u64;
        let mut user_time = 0u64;
        let ok = GetProcessTimes(
            h_shell,
            &mut shell_creation as *mut _ as *mut _,
            &mut exit_time as *mut _ as *mut _,
            &mut kernel_time as *mut _ as *mut _,
            &mut user_time as *mut _ as *mut _,
        );
        CloseHandle(h_shell);

        if ok != 0 {
            // 获取当前系统时间 (FILETIME 为 100ns 计数单位)
            let mut now_ft = 0u64;
            GetSystemTimeAsFileTime(&mut now_ft as *mut _ as *mut _);
            // 计算 explorer.exe 启动至今经过的毫秒数 (10,000 个 100ns = 1ms)
            let shell_uptime_ms = now_ft.saturating_sub(shell_creation) / 10_000;
            // 如果桌面 Shell 启动在 180 秒 (3分钟) 以内，说明是开机自启会话阶段！
            if shell_uptime_ms < 180_000 {
                return true;
            }
        }
    }
    false
}
