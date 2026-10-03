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
    // 1. 全局单实例互斥锁，防止多开
    #[cfg(windows)]
    let _single_instance_guard = match acquire_single_instance_mutex() {
        Ok(handle) => handle,
        Err(_) => {
            eprintln!("NetTrigger 已经在运行中。");
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

    if created {
        println!("✨ 首次启动，已在当前目录下自动生成带详细中文注释的 config.toml 配置文件。");
    }

    let app_name = config.general.app_name.clone();

    // 3. 初始化极轻量系统托盘管理器
    let mut tray_mgr = match SystemTrayManager::new(&app_name, config.general.silent_mode) {
        Ok(mgr) => mgr,
        Err(err) => {
            eprintln!("初始化托盘失败: {}", err);
            return;
        }
    };

    // 4. 初始化核心有限状态机
    let mut fsm = StateMachine::new(config.clone());

    // 5. 启动 Windows 原生 IP Helper 网络状态被动监听器
    let (net_tx, net_rx) = channel();
    let debounce_ms = config.general.effective_debounce_ms();
    let mut watcher_handle = match start_network_watcher(debounce_ms, net_tx) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("启动网络监听器失败 (回退至纯定时器模式): {}", err);
            // 依然保证程序运行，不崩溃
            watcher::win_network::start_network_watcher(150, channel().0).unwrap_or_else(|_| {
                panic_free_fallback_handle()
            })
        }
    };

    // 首次开机自检
    let initial_state = fsm.step_probe();
    let snap = fsm.snapshot();
    tray_mgr.update_state(initial_state, snap.latency_ms, &app_name);

    // 6. 顶层无损事件主循环 (消息泵 + 内核事件 + 心跳定时器)
    let mut last_heartbeat = Instant::now();

    loop {
        // A. 处理 Win32 消息循环（驱动托盘交互，超时 100ms 无损挂起，CPU 0.00%）
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG,
                PM_REMOVE, QS_ALLINPUT,
            };

            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // 内核等待，最多挂起 100ms
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 100, QS_ALLINPUT);
        }

        #[cfg(not(windows))]
        std::thread::sleep(Duration::from_millis(100));

        // B. 处理托盘右键菜单用户操作
        if let Some(action) = tray_mgr.poll_menu_event() {
            match action {
                TrayAction::ManualCheck => {
                    let st = fsm.on_user_manual_trigger();
                    let s = fsm.snapshot();
                    tray_mgr.update_state(st, s.latency_ms, &app_name);
                }
                TrayAction::TestConfig => {
                    // 后台一键诊断
                    let cfg = fsm.config().clone();
                    std::thread::spawn(move || {
                        let ctx = config::MacroContext::build(&cfg);
                        let url = cfg.get_expanded_action_url(&ctx);
                        let params = cfg.get_expanded_params_with_ctx(&ctx);
                        let _ = auth::http_client::execute_http_auth(&cfg.auth.http, &url, &params);
                    });
                }
                TrayAction::OpenPortal => {
                    let portal_url = fsm.config().auth.portal_url.clone();
                    let _ = auth::browser::open_browser_portal(&portal_url, true, true);
                }
                TrayAction::OpenWebConfig => {
                    let current_cfg = fsm.config().clone();
                    let _ = web_config::launch_ephemeral_web_config(current_cfg);
                }
                TrayAction::EditConfigFile => {
                    let config_file = get_config_path();
                    let _ = std::process::Command::new("notepad.exe")
                        .arg(config_file)
                        .spawn();
                }
                TrayAction::ToggleAutostart => {
                    let current_enabled = tray::autostart::is_autostart_enabled();
                    let next_state = !current_enabled;
                    let _ = tray::autostart::set_autostart(next_state);
                    tray_mgr.sync_autostart_state(next_state);
                }
                TrayAction::ToggleSilentMode => {
                    let mut cfg = fsm.config().clone();
                    cfg.general.silent_mode = !cfg.general.silent_mode;
                    if let Ok(toml_str) = toml::to_string_pretty(&cfg) {
                        let _ = std::fs::write(get_config_path(), toml_str);
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

        // C. 处理 Windows 底层网卡变动事件（近乎 0 延迟响应）
        while let Ok(net_evt) = net_rx.try_recv() {
            if net_evt == NetworkEvent::NetworkChanged {
                let st = fsm.on_network_changed();
                let s = fsm.snapshot();
                tray_mgr.update_state(st, s.latency_ms, &app_name);
            }
        }

        // D. 处理低频心跳定时器 (Heartbeat)
        let heartbeat_interval = Duration::from_secs(fsm.config().general.effective_heartbeat_interval_sec());
        if last_heartbeat.elapsed() >= heartbeat_interval {
            last_heartbeat = Instant::now();
            let st = fsm.on_heartbeat_tick();
            let s = fsm.snapshot();
            tray_mgr.update_state(st, s.latency_ms, &app_name);
        }
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

fn panic_free_fallback_handle() -> watcher::NetworkWatcherHandle {
    let (tx, _) = channel();
    watcher::win_network::start_network_watcher(150, tx)
        .unwrap_or_else(|_| std::process::exit(1))
}
