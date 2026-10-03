//! 极轻量 Windows 系统托盘交互模块
//!
//! 核心设计：
//! 1. 托盘图标通过代码在内存中动态生成 16x16 高辨识度 RGBA 矢量圆环，零外部 .ico 文件依赖；
//! 2. 状态四态视觉指示（🟢 在线、🟡 认证中、🔴 离线、⚪ 挂起）；
//! 3. 极速响应的上下文菜单（一键重连、一键测试、按需 Web 配置、自启切换、退出）；
//! 4. 动态 Tooltip 实时反馈延迟与运行状态。

pub mod autostart;

use crate::state::NetworkState;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// 托盘菜单动作命令
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ManualCheck,
    TestConfig,
    OpenPortal,
    OpenWebConfig,
    EditConfigFile,
    ToggleAutostart,
    ToggleSilentMode,
    Quit,
}

pub struct SystemTrayManager {
    tray_icon: TrayIcon,
    _menu: Menu,
    autostart_item: CheckMenuItem,
    silent_item: CheckMenuItem,
    item_manual_check: MenuItem,
    item_test_config: MenuItem,
    item_open_portal: MenuItem,
    item_open_web_config: MenuItem,
    item_edit_config: MenuItem,
    item_quit: MenuItem,
}

impl SystemTrayManager {
    /// 初始化系统托盘图标与菜单
    pub fn new(app_name: &str, is_silent: bool) -> Result<Self, String> {
        let menu = Menu::new();

        let item_manual_check = MenuItem::new("⚡ 立即检测并重新认证", true, None);
        let item_test_config = MenuItem::new("🧪 一键测试当前认证配置", true, None);
        let item_open_portal = MenuItem::new("🌐 打开网络认证网页 (Browser)", true, None);
        let item_open_web_config = MenuItem::new("⚙️ 可视化配置中心 (Web)", true, None);
        let item_edit_config = MenuItem::new("📝 直接编辑 config.toml", true, None);

        let is_auto = autostart::is_autostart_enabled();
        let autostart_item = CheckMenuItem::new("开机自启动", true, is_auto, None);
        let silent_item = CheckMenuItem::new("静默模式 (游戏免打扰)", true, is_silent, None);

        let item_quit = MenuItem::new("🚪 退出 NetTrigger", true, None);

        // 组装上下文菜单
        let _ = menu.append(&item_manual_check);
        let _ = menu.append(&item_test_config);
        let _ = menu.append(&item_open_portal);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_open_web_config);
        let _ = menu.append(&item_edit_config);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&autostart_item);
        let _ = menu.append(&silent_item);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&item_quit);

        let initial_icon = generate_color_icon(NetworkState::Initializing)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip(format!("{} - 初始化中...", app_name))
            .with_icon(initial_icon)
            .build()
            .map_err(|e| format!("构建系统托盘图标失败: {}", e))?;

        Ok(Self {
            tray_icon,
            _menu: menu,
            autostart_item,
            silent_item,
            item_manual_check,
            item_test_config,
            item_open_portal,
            item_open_web_config,
            item_edit_config,
            item_quit,
        })
    }

    /// 更新托盘状态与图标色彩
    pub fn update_state(&mut self, state: NetworkState, latency_ms: u64, app_name: &str) {
        if let Ok(icon) = generate_color_icon(state) {
            let _ = self.tray_icon.set_icon(Some(icon));
        }

        let tooltip = match state {
            NetworkState::Online => {
                format!("{} - 网络畅通 | 延迟: {}ms", app_name, latency_ms)
            }
            NetworkState::Authenticating => {
                format!("{} - 正在自动认证...", app_name)
            }
            NetworkState::CaptivePortal => {
                format!("{} - 检测到网络拦截，准备重连", app_name)
            }
            NetworkState::Disconnected => {
                format!("{} - 网络已断开", app_name)
            }
            NetworkState::BackoffWait => {
                format!("{} - 等待重试冷却中...", app_name)
            }
            NetworkState::Initializing => {
                format!("{} - 正在初始化...", app_name)
            }
        };

        let _ = self.tray_icon.set_tooltip(Some(tooltip));
    }

    /// 轮询托盘菜单用户点击事件（非阻塞）
    pub fn poll_menu_event(&self) -> Option<TrayAction> {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.item_manual_check.id() {
                return Some(TrayAction::ManualCheck);
            } else if event.id == self.item_test_config.id() {
                return Some(TrayAction::TestConfig);
            } else if event.id == self.item_open_portal.id() {
                return Some(TrayAction::OpenPortal);
            } else if event.id == self.item_open_web_config.id() {
                return Some(TrayAction::OpenWebConfig);
            } else if event.id == self.item_edit_config.id() {
                return Some(TrayAction::EditConfigFile);
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
}
