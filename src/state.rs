//! 核心有限状态机 (FSM) 与调度中枢
//!
//! 核心设计：
//! 1. 维护单一可信状态源 (Single Source of Truth)；
//! 2. 串联 Watcher、Probe、Auth 与 Heartbeat；
//! 3. 采用带有网卡变动瞬时重置的指数退避防风暴算法；
//! 4. 0 panic，所有异常安全消化在状态流转中。

use crate::auth::{AuthExecutor, AuthResult};
use crate::config::Config;
use crate::probe::{Probe, ProbeReport, ProbeStatus};
use std::time::{Duration, Instant};

/// 系统核心网络运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    /// 系统启动中 / 首次探测中
    Initializing,
    /// 互联网完全畅通 (204 验证成功)
    Online,
    /// 处于网关拦截状态 (需要 Captive Portal 认证)
    CaptivePortal,
    /// 正在发送认证报文
    Authenticating,
    /// 物理链路断开 / 无网络连接
    Disconnected,
    /// 连续失败，进入指数退避休眠状态
    BackoffWait,
}

impl NetworkState {
    pub fn description(&self) -> &'static str {
        match self {
            NetworkState::Initializing => "正在启动...",
            NetworkState::Online => "网络已连接",
            NetworkState::CaptivePortal => "需要登录校园网",
            NetworkState::Authenticating => "正在连接网络...",
            NetworkState::Disconnected => "未连接到网络 (WiFi/网线未插)",
            NetworkState::BackoffWait => "稍后自动重试",
        }
    }
}

/// 状态机运行快照（供托盘 Tooltip 与诊断界面展示）
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub state: NetworkState,
    pub latency_ms: u64,
    pub consecutive_failures: u32,
    pub uptime_sec: u64,
    pub backoff_remaining_sec: u64,
    pub last_message: String,
}

/// 核心有限状态机
pub struct StateMachine {
    config: Config,
    probe: Probe,
    current_state: NetworkState,
    latency_ms: u64,
    consecutive_failures: u32,
    start_time: Instant,
    last_state_change: Instant,
    backoff_until: Option<Instant>,
    last_message: String,
    last_detected_portal_url: Option<String>,
    last_browser_open_at: Option<Instant>,
}

impl StateMachine {
    /// 创建状态机实例
    pub fn new(config: Config) -> Self {
        let probe = Probe::with_portal(&config.probe, Some(&config.auth.portal_url));
        Self {
            config,
            probe,
            current_state: NetworkState::Initializing,
            latency_ms: 0,
            consecutive_failures: 0,
            start_time: Instant::now(),
            last_state_change: Instant::now(),
            backoff_until: None,
            last_message: "系统已启动".to_string(),
            last_detected_portal_url: None,
            last_browser_open_at: None,
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> NetworkState {
        self.current_state
    }

    /// 获取当前生效配置的只读引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 热重载新配置
    pub fn update_config(&mut self, new_config: Config) {
        self.probe = Probe::with_portal(&new_config.probe, Some(&new_config.auth.portal_url));
        self.config = new_config;
        self.last_message = "配置已动态重载".to_string();
    }

    /// 获取最近一次探针拦截捕获到的网关真实认证重定向地址
    pub fn last_detected_portal_url(&self) -> Option<&str> {
        self.last_detected_portal_url.as_deref()
    }

    /// 获取当前最可靠的登录网址 (优先使用网关动态返回的真实地址，彻底规避 success.jsp 与 IP 漂移)
    pub fn get_effective_portal_url(&self) -> String {
        if let Some(ref detected) = self.last_detected_portal_url {
            if !detected.trim().is_empty() && !detected.contains("success.jsp") {
                return detected.trim().to_string();
            }
        }
        let configured = self.config.auth.portal_url.trim();
        if !configured.is_empty()
            && !configured.contains("success.jsp")
            && !configured.contains("example.")
            && configured != "http://10.10.200.102/"
            && configured != "http://10.10.200.102"
        {
            // 防踩坑：如果配置的 URL 带有静态旧参数（如旧的 wlanuserip=），会导致校园网网关报“原ip与当前用户不一致”
            if configured.contains("wlanuserip=") {
                if let Some(fresh_url) = crate::auth::http_client::get_fresh_ruijie_portal_url() {
                    return fresh_url;
                }
                return "http://123.123.123.123/".to_string();
            }
            configured.to_string()
        } else {
            if let Some(fresh_url) = crate::auth::http_client::get_fresh_ruijie_portal_url() {
                fresh_url
            } else {
                "http://123.123.123.123/".to_string()
            }
        }
    }

    /// 获取状态机运行状态快照
    pub fn snapshot(&self) -> StateSnapshot {
        let now = Instant::now();
        let backoff_remaining_sec = self
            .backoff_until
            .and_then(|until| {
                if until > now {
                    Some(until.duration_since(now).as_secs())
                } else {
                    None
                }
            })
            .unwrap_or(0);

        StateSnapshot {
            state: self.current_state,
            latency_ms: self.latency_ms,
            consecutive_failures: self.consecutive_failures,
            uptime_sec: self.start_time.elapsed().as_secs(),
            backoff_remaining_sec,
            last_message: self.last_message.clone(),
        }
    }

    /// 核心处理：网络状态全面探查与跃迁
    pub fn step_probe(&mut self) -> NetworkState {
        let report = self.probe.check_with_report();
        self.latency_ms = report.latency_ms;
        self.apply_probe_result(report)
    }

    /// 应用探针检测结果并执行状态跃迁
    fn apply_probe_result(&mut self, report: ProbeReport) -> NetworkState {
        match report.status {
            ProbeStatus::Online => {
                // 互联网完全畅通，重置所有退避与失败计数器
                self.consecutive_failures = 0;
                self.backoff_until = None;
                self.last_browser_open_at = None;
                self.transition_to(
                    NetworkState::Online,
                    format!("网络已连接 (延迟: {}ms)", self.latency_ms),
                );
            }
            ProbeStatus::CaptivePortal { redirect_url } => {
                // 智能自动捕获：若网关返回了真实认证地址，且当前用户尚未配置或为占位地址或受污染的 success.jsp，自动自愈采纳
                if let Some(ref detected_url) = redirect_url {
                    if !detected_url.is_empty() {
                        self.last_detected_portal_url = Some(detected_url.clone());
                    }
                    let cur_portal = self.config.auth.portal_url.trim();
                    let is_empty_or_default_or_stale = cur_portal.is_empty()
                        || cur_portal.contains("10.0.0.55")
                        || cur_portal.contains("example.com")
                        || cur_portal.contains("example.edu")
                        || cur_portal.contains("success.jsp");

                    if is_empty_or_default_or_stale && !detected_url.is_empty() {
                        self.config.auth.portal_url = detected_url.clone();
                        let cfg_to_save = self.config.clone();
                        std::thread::spawn(move || {
                            if let Ok(toml_str) = toml::to_string_pretty(&cfg_to_save) {
                                let _ = std::fs::write(crate::utils::fs::get_config_path(), toml_str);
                            }
                        });
                        println!("✨ 智能识别并自动设置校园网登录网址: {}", detected_url);
                    }
                }

                let msg = "需要登录校园网，正在准备连接...".to_string();
                self.transition_to(NetworkState::CaptivePortal, msg);

                // 立即触发自动重连动作
                self.trigger_reconnect();
            }
            ProbeStatus::Offline { reason } => {
                self.transition_to(NetworkState::Disconnected, format!("未连接到网络: {}", reason));
            }
        }

        self.current_state
    }

    /// 触发自动重连动作流水线
    pub fn trigger_reconnect(&mut self) {
        // 检查退避窗口
        if let Some(until) = self.backoff_until {
            if Instant::now() < until {
                // 仍处于退避冷却中，暂不发包
                return;
            }
        }

        // 关键防护：若配置为局域网认证（如校园网 10.10.200.102 / eportal），但物理局域网网卡未连接（Wi-Fi已断开或网线未插）
        let is_campus_target = self.config.auth.http.action_url.contains("10.10.200.102")
            || self.config.auth.portal_url.contains("10.10.200.102")
            || self.config.auth.http.action_url.contains("eportal");

        if is_campus_target && crate::config::get_lan_adapter_ipv4().is_none() {
            // 物理局域网未连接，绝不可向 4G 蜂窝网卡发校园网认证包！
            // 立即核验当前机器是否有其它网络连通（例如 LTE 蜂窝网络正常上网）
            let probe_rep = self.probe.check_with_report();
            if probe_rep.status.is_online() {
                self.consecutive_failures = 0;
                self.backoff_until = None;
                self.transition_to(
                    NetworkState::Online,
                    format!("网络已连接 (延迟: {}ms)", probe_rep.latency_ms),
                );
            } else {
                self.transition_to(
                    NetworkState::Disconnected,
                    "未连接到网络 (WiFi未开启/网线未插)".to_string(),
                );
            }
            return;
        }

        let is_browser_mode = self.config.auth.mode == "browser";
        let action_desc = if is_browser_mode {
            "正在打开登录网页..."
        } else {
            "正在自动连接网络..."
        };
        self.transition_to(NetworkState::Authenticating, action_desc.to_string());

        let detected_url = self.last_detected_portal_url.as_deref();

        // 浏览器模式下防频繁重复弹窗切屏：
        // 若在最近 30 秒内已经唤起过浏览器窗口，且正处于退避等待中，避免频繁重复弹窗切屏
        let should_skip_browser_open = if is_browser_mode {
            if let Some(opened_at) = self.last_browser_open_at {
                opened_at.elapsed() < Duration::from_secs(30) && self.consecutive_failures > 0
            } else {
                false
            }
        } else {
            false
        };

        let auth_result: AuthResult = if should_skip_browser_open {
            AuthResult::ok(200, "等待用户在已打开的网页中完成登录...".to_string())
        } else {
            let res = AuthExecutor::execute(&self.config, detected_url);
            if is_browser_mode && res.success {
                self.last_browser_open_at = Some(Instant::now());
            }
            res
        };

        if is_browser_mode {
            // 网页自动登录模式下，给浏览器 1.5 秒启动、自动填充与提交时间，避免 0ms 瞬间误判失败
            std::thread::sleep(Duration::from_millis(1500));
        } else if auth_result.success {
            // 校园网网关（如锐捷 SAM+、深澜等）在收到登录成功响应后，底层防火墙规则下发通常有 100~300ms 纳管延迟
            // 稍作缓冲后再进行权威探测核验，杜绝瞬间误判
            std::thread::sleep(Duration::from_millis(300));
        }

        // 认证报文发送完成后，执行一次快速探针二次核验
        let verify_report = self.probe.check_with_report();
        self.latency_ms = verify_report.latency_ms;

        if verify_report.status.is_online() {
            self.consecutive_failures = 0;
            self.backoff_until = None;
            self.last_browser_open_at = None;
            self.transition_to(
                NetworkState::Online,
                format!("网络连接成功！(延迟: {}ms)", self.latency_ms),
            );
        } else {
            // 认证仍未成功，启动平滑退避
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let backoff_secs = self.calculate_backoff_secs();
            self.backoff_until = Some(Instant::now() + Duration::from_secs(backoff_secs));

            let reason_prefix = if !auth_result.success && !auth_result.message.is_empty() {
                format!("{}: ", auth_result.message)
            } else {
                String::new()
            };

            let user_friendly_msg = if is_browser_mode {
                format!(
                    "{}等待网页登录中，{}秒后自动检测 (第{}次)",
                    reason_prefix, backoff_secs, self.consecutive_failures
                )
            } else {
                format!(
                    "{}连接未成功，{}秒后自动重试 (第{}次)",
                    reason_prefix, backoff_secs, self.consecutive_failures
                )
            };
            self.transition_to(NetworkState::BackoffWait, user_friendly_msg);
        }
    }

    /// 响应 Windows 内核网络变动事件（网卡重连/IP跳变）
    ///
    /// 核心特性：瞬时重置所有失败计数与退避计时，以最高优先级瞬时唤醒重新认证！
    pub fn on_network_changed(&mut self) -> NetworkState {
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.last_browser_open_at = None;
        self.last_message = "网络已变动，立即重新检测...".to_string();
        self.step_probe()
    }

    /// 响应心跳时钟滴答 (低频保活探测)
    pub fn on_heartbeat_tick(&mut self) -> NetworkState {
        // 若当前处于退避等待中，检查退避是否已到期
        if let Some(until) = self.backoff_until {
            if Instant::now() >= until {
                self.backoff_until = None;
                return self.step_probe();
            }
            return self.current_state;
        }

        self.step_probe()
    }

    /// 用户主动发起立即检测/重连（托盘菜单触发）
    pub fn on_user_manual_trigger(&mut self) -> NetworkState {
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.last_browser_open_at = None;
        self.last_message = "用户手动触发网络检测与重连".to_string();
        let state = self.step_probe();
        if state != NetworkState::Online {
            self.trigger_reconnect();
        }
        self.current_state
    }

    /// 状态跃迁辅助函数
    fn transition_to(&mut self, new_state: NetworkState, message: String) {
        if self.current_state != new_state {
            self.current_state = new_state;
            self.last_state_change = Instant::now();
        }
        self.last_message = message;
    }

    /// 计算当前失败次数下的指数退避秒数 (1s -> 2s -> 4s -> 8s -> 16s -> 30s -> 60s)
    fn calculate_backoff_secs(&self) -> u64 {
        let max_backoff = self.config.general.max_backoff_sec.max(5);
        let base_delay = 1u64;
        let shift = self.consecutive_failures.saturating_sub(1).min(6);
        let computed = base_delay * (1 << shift);
        computed.min(max_backoff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_calculation() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);

        fsm.consecutive_failures = 1;
        assert_eq!(fsm.calculate_backoff_secs(), 1);

        fsm.consecutive_failures = 2;
        assert_eq!(fsm.calculate_backoff_secs(), 2);

        fsm.consecutive_failures = 3;
        assert_eq!(fsm.calculate_backoff_secs(), 4);

        fsm.consecutive_failures = 4;
        assert_eq!(fsm.calculate_backoff_secs(), 8);

        fsm.consecutive_failures = 5;
        assert_eq!(fsm.calculate_backoff_secs(), 16);

        fsm.consecutive_failures = 6;
        assert_eq!(fsm.calculate_backoff_secs(), 32);

        fsm.consecutive_failures = 7;
        assert_eq!(fsm.calculate_backoff_secs(), 60);

        fsm.consecutive_failures = 100;
        assert_eq!(fsm.calculate_backoff_secs(), 60);
    }

    #[test]
    fn test_network_changed_resets_backoff() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);

        fsm.consecutive_failures = 5;
        fsm.backoff_until = Some(Instant::now() + Duration::from_secs(60));

        // 模拟网络事件唤醒
        fsm.on_network_changed();
        assert_eq!(fsm.consecutive_failures, 0);
        assert!(fsm.backoff_until.is_none());
    }

    #[test]
    fn test_get_effective_portal_url_sanitizes_success_jsp() {
        let mut config = Config::default();
        config.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=abc".to_string();
        let fsm = StateMachine::new(config);

        assert_eq!(fsm.get_effective_portal_url(), "http://123.123.123.123/");
    }

    #[test]
    fn test_get_effective_portal_url_uses_detected_url() {
        let mut config = Config::default();
        config.auth.portal_url = "http://10.10.200.102/eportal/success.jsp?userIndex=abc".to_string();
        let mut fsm = StateMachine::new(config);
        fsm.last_detected_portal_url = Some("http://10.10.200.102/eportal/index.jsp?wlanuserip=1.2.3.4".to_string());

        assert_eq!(fsm.get_effective_portal_url(), "http://10.10.200.102/eportal/index.jsp?wlanuserip=1.2.3.4");
    }
}
