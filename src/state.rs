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
            NetworkState::Initializing => "初始化中...",
            NetworkState::Online => "网络畅通",
            NetworkState::CaptivePortal => "需要认证",
            NetworkState::Authenticating => "正在认证中...",
            NetworkState::Disconnected => "网络已断开",
            NetworkState::BackoffWait => "重试等待中",
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
}

impl StateMachine {
    /// 创建状态机实例
    pub fn new(config: Config) -> Self {
        let probe = Probe::new(&config.probe);
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
        self.probe = Probe::new(&new_config.probe);
        self.config = new_config;
        self.last_message = "配置已动态重载".to_string();
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
                self.transition_to(
                    NetworkState::Online,
                    format!("网络畅通 (延迟: {}ms)", self.latency_ms),
                );
            }
            ProbeStatus::CaptivePortal { redirect_url } => {
                let msg = redirect_url
                    .map(|u| format!("检测到网络认证拦截: {}", u))
                    .unwrap_or_else(|| "检测到网关认证拦截".to_string());
                self.transition_to(NetworkState::CaptivePortal, msg);

                // 立即触发自动重连动作
                self.trigger_reconnect();
            }
            ProbeStatus::Offline { reason } => {
                self.transition_to(NetworkState::Disconnected, format!("网络断开: {}", reason));
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

        self.transition_to(NetworkState::Authenticating, "正在发起后台静默认证...".to_string());

        // 执行认证
        let auth_result: AuthResult = AuthExecutor::execute(&self.config);

        // 认证报文发送完成后，立即执行一次快速探针二次核验
        let verify_report = self.probe.check_with_report();
        self.latency_ms = verify_report.latency_ms;

        if verify_report.status.is_online() {
            self.consecutive_failures = 0;
            self.backoff_until = None;
            self.transition_to(
                NetworkState::Online,
                format!("自动认证成功！(延迟: {}ms)", self.latency_ms),
            );
        } else {
            // 认证仍未成功，启动指数退避防风暴
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let backoff_secs = self.calculate_backoff_secs();
            self.backoff_until = Some(Instant::now() + Duration::from_secs(backoff_secs));

            let fail_msg = format!(
                "认证未通过: {}。{}秒后重试 (第{}次)",
                auth_result.message, backoff_secs, self.consecutive_failures
            );
            self.transition_to(NetworkState::BackoffWait, fail_msg);
        }
    }

    /// 响应 Windows 内核网络变动事件（网卡重连/IP跳变）
    ///
    /// 核心特性：瞬时重置所有失败计数与退避计时，以最高优先级瞬时唤醒重新认证！
    pub fn on_network_changed(&mut self) -> NetworkState {
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.last_message = "收到 Windows 网络变动事件，立即重新检测...".to_string();
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
        self.last_message = "用户手动触发网络检测".to_string();
        self.step_probe()
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
}
