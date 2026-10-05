//! 极轻量 Windows 系统托盘交互模块
//!
//! 核心设计：
//! 1. 托盘图标通过代码在内存中动态生成 16x16 高辨识度 RGBA 矢量圆环，零外部 .ico 文件依赖；
//! 2. 状态四态视觉指示（🟢 在线、🟡 认证中、🔴 离线、⚪ 挂起）；
//! 3. 极速响应的上下文菜单（一键重连、一键测试、按需 Web 配置、自启切换、退出）；
//! 4. 动态 Tooltip 实时反馈延迟与运行状态。

pub mod autostart;

use crate::config::Language;
use crate::state::NetworkState;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// 托盘多语言文本静态映射表（零动态分配）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayI18n {
    pub manual_check: &'static str,
    pub open_portal: &'static str,
    pub diagnostic_report: &'static str,
    pub settings: &'static str,
    pub autostart: &'static str,
    pub silent_mode: &'static str,
    pub quit: &'static str,
    pub status_online: &'static str,
    pub status_authenticating: &'static str,
    pub status_captive_portal: &'static str,
    pub status_disconnected: &'static str,
    pub status_backoff: &'static str,
    pub status_initializing: &'static str,
    pub latency_prefix: &'static str,
    pub init_tooltip: &'static str,
}

pub const I18N_ZH: TrayI18n = TrayI18n {
    manual_check: "立即重连",
    open_portal: "打开登录网页",
    diagnostic_report: "网络排障诊断报告",
    settings: "设置",
    autostart: "开机自启动",
    silent_mode: "静默模式 (免打扰)",
    quit: "退出",
    status_online: "网络已连接",
    status_authenticating: "正在连接网络...",
    status_captive_portal: "网络需要登录，正在自动连接...",
    status_disconnected: "未连接到网络 (WiFi/网线未插)",
    status_backoff: "网络连接稍后重试...",
    status_initializing: "正在启动...",
    latency_prefix: "延迟",
    init_tooltip: "初始化中...",
};

pub const I18N_EN: TrayI18n = TrayI18n {
    manual_check: "Reconnect Now",
    open_portal: "Open Login Portal",
    diagnostic_report: "Diagnostic Report",
    settings: "Settings",
    autostart: "Start on Boot",
    silent_mode: "Silent Mode (Do Not Disturb)",
    quit: "Quit",
    status_online: "Connected",
    status_authenticating: "Connecting...",
    status_captive_portal: "Captive portal detected, connecting...",
    status_disconnected: "Disconnected (No Wi-Fi/Ethernet)",
    status_backoff: "Retry in cooldown...",
    status_initializing: "Starting up...",
    latency_prefix: "Latency",
    init_tooltip: "Initializing...",
};

impl TrayI18n {
    pub fn get(lang: Language) -> &'static TrayI18n {
        match lang {
            Language::Zh => &I18N_ZH,
            Language::En => &I18N_EN,
            Language::Auto => match crate::config::detect_system_language() {
                Language::Zh => &I18N_ZH,
                _ => &I18N_EN,
            },
        }
    }
}

/// 托盘菜单动作命令
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ManualCheck,
    OpenPortal,
    OpenDiagnosticReport,
    OpenWebConfig,
    ToggleAutostart,
    ToggleSilentMode,
    SetLanguage(Language),
    Quit,
}

pub struct SystemTrayManager {
    tray_icon: TrayIcon,
    _menu: Menu,
    autostart_item: CheckMenuItem,
    silent_item: CheckMenuItem,
    item_manual_check: MenuItem,
    item_open_portal: MenuItem,
    item_diagnostic_report: MenuItem,
    item_open_web_config: MenuItem,
    item_lang_auto: CheckMenuItem,
    item_lang_en: CheckMenuItem,
    item_lang_zh: CheckMenuItem,
    item_quit: MenuItem,
    language: Language,
    last_state: NetworkState,
    last_latency_ms: u64,
    last_message: String,
}

impl SystemTrayManager {
    /// 初始化系统托盘图标与菜单
    pub fn new(app_name: &str, is_silent: bool, lang: Language) -> Result<Self, String> {
        let menu = Menu::new();
        let i18n = TrayI18n::get(lang);

        let item_manual_check = MenuItem::new(i18n.manual_check, true, None);
        let item_open_portal = MenuItem::new(i18n.open_portal, true, None);
        let item_diagnostic_report = MenuItem::new(i18n.diagnostic_report, true, None);
        let item_open_web_config = MenuItem::new(i18n.settings, true, None);

        // 原生二级语言子菜单：常驻 "Language / 语言"，保证任何语言用户第一眼即可辨识
        let lang_submenu = Submenu::new("Language / 语言", true);
        let item_lang_auto = CheckMenuItem::new("Auto (自动)", true, lang == Language::Auto, None);
        let item_lang_en = CheckMenuItem::new("English", true, lang == Language::En, None);
        let item_lang_zh = CheckMenuItem::new("简体中文", true, lang == Language::Zh, None);

        let _ = lang_submenu.append(&item_lang_auto);
        let _ = lang_submenu.append(&item_lang_en);
        let _ = lang_submenu.append(&item_lang_zh);

        let is_auto = autostart::is_autostart_enabled();
        let autostart_item = CheckMenuItem::new(i18n.autostart, true, is_auto, None);
        let silent_item = CheckMenuItem::new(i18n.silent_mode, true, is_silent, None);

        let item_quit = MenuItem::new(i18n.quit, true, None);

        // 组装极简无冗余的上下文菜单
        let _ = menu.append(&item_manual_check);
        let _ = menu.append(&item_open_portal);
        let _ = menu.append(&item_diagnostic_report);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_open_web_config);
        let _ = menu.append(&lang_submenu);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&autostart_item);
        let _ = menu.append(&silent_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_quit);

        let initial_icon = generate_color_icon(NetworkState::Initializing)?;

        // 开机自启动或快速重启时，Windows 任务栏通知区域可能尚未就绪 (E_FAIL / -2147467259)
        // 采用轻量重试机制平滑等待 Windows Explorer Shell 托盘就绪
        let mut retry_count = 0;
        let tray_icon = loop {
            match TrayIconBuilder::new()
                .with_menu(Box::new(menu.clone()))
                .with_tooltip(format!("{} - {}", app_name, i18n.init_tooltip))
                .with_icon(initial_icon.clone())
                .with_menu_on_left_click(true)
                .build()
            {
                Ok(icon) => break icon,
                Err(_e) if retry_count < 10 => {
                    retry_count += 1;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(e) => return Err(format!("构建系统托盘图标失败: {}", e)),
            }
        };

        Ok(Self {
            tray_icon,
            _menu: menu,
            autostart_item,
            silent_item,
            item_manual_check,
            item_open_portal,
            item_diagnostic_report,
            item_open_web_config,
            item_lang_auto,
            item_lang_en,
            item_lang_zh,
            item_quit,
            language: lang,
            last_state: NetworkState::Initializing,
            last_latency_ms: 0,
            last_message: String::new(),
        })
    }

    /// 更新界面语言并即时重置菜单文本
    pub fn set_language(&mut self, lang: Language, app_name: &str) {
        self.language = lang;
        self.item_lang_auto.set_checked(lang == Language::Auto);
        self.item_lang_en.set_checked(lang == Language::En);
        self.item_lang_zh.set_checked(lang == Language::Zh);

        let i18n = TrayI18n::get(lang);
        self.item_manual_check.set_text(i18n.manual_check);
        self.item_open_portal.set_text(i18n.open_portal);
        self.item_diagnostic_report.set_text(i18n.diagnostic_report);
        self.item_open_web_config.set_text(i18n.settings);
        self.autostart_item.set_text(i18n.autostart);
        self.silent_item.set_text(i18n.silent_mode);
        self.item_quit.set_text(i18n.quit);
        let msg = self.last_message.clone();
        self.update_state(self.last_state, self.last_latency_ms, &msg, app_name);
    }

    /// 更新托盘状态与图标色彩，并根据细粒度状态实时更新气泡提示
    pub fn update_state(&mut self, state: NetworkState, latency_ms: u64, last_message: &str, app_name: &str) {
        self.last_state = state;
        self.last_latency_ms = latency_ms;
        self.last_message = last_message.to_string();
        if let Ok(icon) = generate_color_icon(state) {
            let _ = self.tray_icon.set_icon(Some(icon));
        }

        let i18n = TrayI18n::get(self.language);
        let tooltip = match state {
            NetworkState::Online => {
                format!(
                    "{} - {} | {}: {}ms",
                    app_name, i18n.status_online, i18n.latency_prefix, latency_ms
                )
            }
            NetworkState::Authenticating => {
                if !last_message.is_empty() {
                    format!("{} - 🟡 {}", app_name, last_message)
                } else {
                    format!("{} - {}", app_name, i18n.status_authenticating)
                }
            }
            NetworkState::CaptivePortal => {
                if !last_message.is_empty() {
                    format!("{} - 🟡 {}", app_name, last_message)
                } else {
                    format!("{} - {}", app_name, i18n.status_captive_portal)
                }
            }
            NetworkState::Disconnected => {
                if !last_message.is_empty() {
                    format!("{} - 🔴 {}", app_name, last_message)
                } else {
                    format!("{} - {}", app_name, i18n.status_disconnected)
                }
            }
            NetworkState::BackoffWait => {
                if !last_message.is_empty() {
                    format!("{} - 🔴 {}", app_name, last_message)
                } else {
                    format!("{} - {}", app_name, i18n.status_backoff)
                }
            }
            NetworkState::Initializing => {
                format!("{} - {}", app_name, i18n.status_initializing)
            }
        };

        // 限制在 Windows NOTIFYICONDATA 127 字符安全上限内，防止系统截断乱码
        let clamped_tooltip = if tooltip.chars().count() > 120 {
            let mut t: String = tooltip.chars().take(117).collect();
            t.push_str("...");
            t
        } else {
            tooltip
        };

        let _ = self.tray_icon.set_tooltip(Some(clamped_tooltip));
    }

    /// 轮询托盘交互事件（非阻塞）
    pub fn poll_menu_event(&self) -> Option<TrayAction> {
        // 1. 响应托盘图标左键双击：直接唤起【设置】
        // 左键单击由 tray-icon 的 menu_on_left_click 自动弹出菜单，避免误跳浏览器
        let mut hit_double_click = false;
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                hit_double_click = true;
                break;
            }
        }

        if hit_double_click {
            // 排空通道内堆积的后续托盘鼠标事件，彻底杜绝连击
            while TrayIconEvent::receiver().try_recv().is_ok() {}
            return Some(TrayAction::OpenWebConfig);
        }

        // 2. 响应右键上下文菜单项点击
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.item_manual_check.id() {
                return Some(TrayAction::ManualCheck);
            } else if event.id == self.item_open_portal.id() {
                return Some(TrayAction::OpenPortal);
            } else if event.id == self.item_diagnostic_report.id() {
                return Some(TrayAction::OpenDiagnosticReport);
            } else if event.id == self.item_open_web_config.id() {
                return Some(TrayAction::OpenWebConfig);
            } else if event.id == self.item_lang_auto.id() {
                return Some(TrayAction::SetLanguage(Language::Auto));
            } else if event.id == self.item_lang_en.id() {
                return Some(TrayAction::SetLanguage(Language::En));
            } else if event.id == self.item_lang_zh.id() {
                return Some(TrayAction::SetLanguage(Language::Zh));
            } else if event.id == self.autostart_item.id() {
                return Some(TrayAction::ToggleAutostart);
            } else if event.id == self.silent_item.id() {
                return Some(TrayAction::ToggleSilentMode);
            } else if event.id == self.item_quit.id() {
                return Some(TrayAction::Quit);
            }
        }
        None
    }

    /// 更新自启菜单勾选状态
    pub fn sync_autostart_state(&self, enabled: bool) {
        self.autostart_item.set_checked(enabled);
    }
}

/// 纯内存动态生成 16x16 矢量发光图标，零外部文件依赖
fn generate_color_icon(state: NetworkState) -> Result<Icon, String> {
    const WIDTH: u32 = 16;
    const HEIGHT: u32 = 16;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);

    let (r, g, b) = match state {
        NetworkState::Online => (34u8, 197u8, 94u8),         // 纯净翠绿 🟢
        NetworkState::Authenticating => (245u8, 158u8, 11u8), // 金黄闪耀 🟡
        NetworkState::CaptivePortal => (245u8, 158u8, 11u8),  // 金黄
        NetworkState::Disconnected => (239u8, 68u8, 68u8),    // 警示红 🔴
        NetworkState::BackoffWait => (239u8, 68u8, 68u8),     // 警示红 🔴
        NetworkState::Initializing => (56u8, 189u8, 248u8),   // 天空蓝 🔵
    };

    let center_x = 7.5f32;
    let center_y = 7.5f32;
    let radius = 6.2f32;

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let dx = (x as f32) - center_x;
            let dy = (y as f32) - center_y;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= radius {
                let alpha = if dist > radius - 1.0 {
                    // 边缘抗锯齿平滑过渡
                    ((radius - dist) * 255.0) as u8
                } else {
                    255u8
                };
                rgba.extend_from_slice(&[r, g, b, alpha]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Icon::from_rgba(rgba, WIDTH, HEIGHT).map_err(|e| format!("生成动态图标失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_generation_succeeds() {
        let icon_res = generate_color_icon(NetworkState::Online);
        assert!(icon_res.is_ok());

        let icon_auth = generate_color_icon(NetworkState::Authenticating);
        assert!(icon_auth.is_ok());

        let icon_disc = generate_color_icon(NetworkState::Disconnected);
        assert!(icon_disc.is_ok());
    }

    #[test]
    fn test_tray_i18n_mappings() {
        let zh = TrayI18n::get(Language::Zh);
        assert_eq!(zh.settings, "设置");
        assert_eq!(zh.manual_check, "立即重连");
        assert_eq!(zh.diagnostic_report, "网络排障诊断报告");

        let en = TrayI18n::get(Language::En);
        assert_eq!(en.settings, "Settings");
        assert_eq!(en.manual_check, "Reconnect Now");
        assert_eq!(en.diagnostic_report, "Diagnostic Report");
    }

}
