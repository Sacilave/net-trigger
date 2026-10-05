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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

/// 细粒度网络诊断记录
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiagnosticRecord {
    pub stage_code: String,
    pub title: String,
    pub detail: String,
    pub suggestion: String,
    pub timestamp: String,
}

/// 状态机运行快照（供托盘 Tooltip 与诊断界面展示）
#[derive(Debug, Clone, serde::Serialize)]
pub struct StateSnapshot {
    pub state: NetworkState,
    pub latency_ms: u64,
    pub consecutive_failures: u32,
    pub uptime_sec: u64,
    pub backoff_remaining_sec: u64,
    pub last_message: String,
    pub last_stage_code: String,
    pub last_suggestion: String,
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
    browser_open_count: u32,
    last_diagnostic: Option<DiagnosticRecord>,
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
            browser_open_count: 0,
            last_diagnostic: None,
        }
    }

    /// 判断当前是否允许自动拉起外部浏览器登录网页
    ///
    /// 核心防抖与防夺焦守则：
    /// 1. 首次触发：0ms 立即放行；
    /// 2. 处于冷却期内：坚决拦截，避免短时间内重复弹窗打开几十个标签页；
    /// 3. 指数退避间隔：第 1 次打开后冷却 60 秒，第 2 次打开后冷却 180 秒；
    /// 4. 最大连续打开 3 次后彻底停止自动弹窗，转为静默守护并引导用户手动点击。
    pub fn should_allow_browser_open(&self) -> bool {
        let opened_at = match self.last_browser_open_at {
            Some(t) => t,
            None => return true,
        };

        if self.browser_open_count >= 3 {
            return false;
        }

        let cooldown = match self.browser_open_count {
            0 => Duration::from_secs(0),
            1 => Duration::from_secs(60),
            _ => Duration::from_secs(180),
        };

        opened_at.elapsed() >= cooldown
    }

    /// 记录实际执行了打开浏览器动作
    pub fn record_browser_opened(&mut self) {
        self.last_browser_open_at = Some(Instant::now());
        self.browser_open_count = self.browser_open_count.saturating_add(1);
    }

    /// 计算浏览器模式下的防重弹等待退避秒数
    pub fn calculate_browser_backoff_secs(&self) -> u64 {
        match self.browser_open_count {
            0 => 60,
            1 => 60,
            2 => 180,
            _ => 300,
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

    /// 记录诊断信息
    pub fn record_diagnostic(
        &mut self,
        stage_code: &str,
        title: &str,
        detail: &str,
        suggestion: &str,
    ) {
        self.last_diagnostic = Some(DiagnosticRecord {
            stage_code: stage_code.to_string(),
            title: title.to_string(),
            detail: detail.to_string(),
            suggestion: suggestion.to_string(),
            timestamp: get_local_time_string(),
        });
    }

    /// 获取最近一次诊断快照
    pub fn last_diagnostic(&self) -> Option<&DiagnosticRecord> {
        self.last_diagnostic.as_ref()
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

        let (last_stage_code, last_suggestion) = if let Some(ref diag) = self.last_diagnostic {
            (diag.stage_code.clone(), diag.suggestion.clone())
        } else {
            ("OK".to_string(), String::new())
        };

        StateSnapshot {
            state: self.current_state,
            latency_ms: self.latency_ms,
            consecutive_failures: self.consecutive_failures,
            uptime_sec: self.start_time.elapsed().as_secs(),
            backoff_remaining_sec,
            last_message: self.last_message.clone(),
            last_stage_code,
            last_suggestion,
        }
    }

    /// 获取探针实例的克隆（供后台工作线程并发探查，彻底不阻塞 UI 消息泵）
    pub fn probe_instance(&self) -> Probe {
        self.probe.clone()
    }

    /// 应用探针检测结果并执行状态跃迁，返回是否需要立即触发重连
    pub fn apply_probe_result(&mut self, report: ProbeReport) -> bool {
        self.latency_ms = report.latency_ms;
        match report.status {
            ProbeStatus::Online => {
                // 互联网完全畅通，重置所有退避与失败计数器
                self.consecutive_failures = 0;
                self.backoff_until = None;
                self.last_browser_open_at = None;
                self.browser_open_count = 0;
                self.record_diagnostic(
                    "OK",
                    "网络畅通已放行",
                    &format!("互联网 204 端点权威校验通过 (延迟: {}ms)", self.latency_ms),
                    "当前互联网连接正常，无需任何操作。",
                );
                self.transition_to(
                    NetworkState::Online,
                    format!("网络已连接 (延迟: {}ms)", self.latency_ms),
                );
                false
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
                        let base_portal = if let Some(q_pos) = detected_url.find('?') {
                            &detected_url[..q_pos]
                        } else {
                            detected_url
                        };
                        self.config.auth.portal_url = base_portal.to_string();
                        let cfg_to_save = self.config.clone();
                        std::thread::spawn(move || {
                            if let Ok(toml_str) = toml::to_string_pretty(&cfg_to_save) {
                                let _ = std::fs::write(crate::utils::fs::get_config_path(), toml_str);
                            }
                        });
                        println!("✨ 智能识别并自动设置校园网登录网址: {}", base_portal);
                    }
                }

                self.record_diagnostic(
                    "E2-01",
                    "检测到校园网 Captive Portal 拦截",
                    &format!("外网已被拦截 (目标跳转: {})", redirect_url.as_deref().unwrap_or("未捕获具体地址")),
                    "请等待 NetTrigger 自动提交认证，或右键托盘点击【立即重连】。",
                );

                let msg = "需要登录校园网，正在准备连接...".to_string();
                self.transition_to(NetworkState::CaptivePortal, msg);
                true
            }
            ProbeStatus::Offline { reason } => {
                self.record_diagnostic(
                    "E1-01",
                    "网络未连接或不可达",
                    &reason,
                    "请检查 Wi-Fi 连接是否已连接到校园网，或检查网线是否插好。",
                );
                self.transition_to(NetworkState::Disconnected, format!("未连接到网络: {}", reason));
                false
            }
        }
    }

    /// 核心处理：网络状态全面探查与跃迁（同步接口）
    pub fn step_probe(&mut self) -> NetworkState {
        let report = self.probe.check_with_report();
        if self.apply_probe_result(report) {
            self.trigger_reconnect();
        }
        self.current_state
    }

    /// 检查并准备重连前置条件（核验局域网链路、DHCP状态等）
    pub fn can_prepare_reconnect(&mut self) -> Result<Option<String>, ()> {
        if let Some(until) = self.backoff_until {
            if Instant::now() < until {
                return Err(());
            }
        }

        let is_campus_target = self.config.auth.http.action_url.contains("10.10.200.102")
            || self.config.auth.portal_url.contains("10.10.200.102")
            || self.config.auth.http.action_url.contains("eportal");

        let lan_ip_opt = crate::config::get_lan_adapter_ipv4();
        if is_campus_target {
            if let Some(ip) = lan_ip_opt {
                let ip_str = ip.to_string();
                if ip_str.starts_with("169.254.") {
                    self.record_diagnostic(
                        "E1-02",
                        "正在等待 DHCP 分配校园网 IP",
                        &format!("当前局域网 IP 为 {} (APIPA 自分配)", ip_str),
                        "Windows 尚未完成校园网 DHCP 租约分配，请稍等 1~2 秒获取 10.x 局域网 IP。",
                    );
                    self.transition_to(
                        NetworkState::BackoffWait,
                        "[E1-02] 网络准备中: 正在获取校园网 IP (DHCP)...".to_string(),
                    );
                    return Err(());
                }
            } else {
                self.record_diagnostic(
                    "E1-01",
                    "未连接网络: WiFi未开启或网线未插",
                    "系统中未检测到活动的以太网或无线局域网适配器",
                    "请在 Windows 中开启 Wi-Fi 并连接到校园网无线热点。",
                );
                self.transition_to(
                    NetworkState::Disconnected,
                    "[E1-01] 未连接到网络 (WiFi未开启/网线未插)".to_string(),
                );
                return Err(());
            }
        }

        let is_browser_mode = self.config.auth.mode == "browser";
        let action_desc = if is_browser_mode {
            if self.should_allow_browser_open() {
                "正在打开登录网页..."
            } else {
                "等待网页端登录完成..."
            }
        } else {
            "正在自动连接网络..."
        };
        self.transition_to(NetworkState::Authenticating, action_desc.to_string());

        Ok(self.last_detected_portal_url.clone())
    }

    /// 应用异步重连完成的结果
    pub fn apply_reconnect_result(&mut self, auth_result: AuthResult, verify_report: ProbeReport) {
        self.latency_ms = verify_report.latency_ms;

        if verify_report.status.is_online() {
            self.consecutive_failures = 0;
            self.backoff_until = None;
            self.last_browser_open_at = None;
            self.browser_open_count = 0;
            self.record_diagnostic(
                "OK",
                "网络已连通",
                &format!("已成功通过 204 端点二次核验 (延迟: {}ms)", self.latency_ms),
                "校园网已成功登录并放行，当前工作正常。",
            );
            self.transition_to(
                NetworkState::Online,
                format!("网络连接成功！(延迟: {}ms)", self.latency_ms),
            );
        } else {
            let is_browser_action = self.config.auth.mode == "browser"
                || auth_result.stage_code == "E5-FALLBACK-BROWSER"
                || auth_result.stage_code == "E5-FALLBACK-WAITING"
                || auth_result.stage_code == "E5-BROWSER-WAITING";

            if is_browser_action {
                // 仅当真正执行了打开浏览器动作时，才递增计数并记录时间戳
                let is_fresh_open = auth_result.stage_code == "E5-FALLBACK-BROWSER"
                    || (self.config.auth.mode == "browser" && auth_result.success);

                if is_fresh_open {
                    self.record_browser_opened();
                }

                // 浏览器模式下采用递增的防抖冷却等待时间，绝不频繁重复弹窗切屏
                let backoff_secs = self.calculate_browser_backoff_secs();
                self.backoff_until = Some(Instant::now() + Duration::from_secs(backoff_secs));

                let prompt_msg = if self.browser_open_count >= 3 {
                    "已多次打开网页，等待登录中 (如未弹出请右键托盘【打开认证网页】)"
                } else if auth_result.stage_code == "E5-FALLBACK-BROWSER" || auth_result.stage_code == "E5-FALLBACK-WAITING" {
                    "[E5-01] 静默登录未成功，已唤起浏览器登录，等待网页端登录完成..."
                } else {
                    "已打开浏览器登录页面，等待网页端登录完成..."
                };
                self.transition_to(NetworkState::BackoffWait, prompt_msg.to_string());
            } else {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                let backoff_secs = self.calculate_backoff_secs();
                self.backoff_until = Some(Instant::now() + Duration::from_secs(backoff_secs));

                // 仅当 HTTP 模拟认证返回了明确成功 (status 200 且 success 为 true)，但外网探针依然被拦截时，才判定为 E6-01 (规则生效延迟)
                if auth_result.success {
                    self.record_diagnostic(
                        "E6-01",
                        "认证已发送但外网尚未放行 (规则延迟)",
                        "网关返回成功，但外网 204 探针仍被拦截 (防火墙规则生效延迟)",
                        "校园网防火墙流表下发通常有 1~3 秒延迟，NetTrigger 将在稍后自动二次核验。",
                    );
                } else {
                    self.record_diagnostic(
                        auth_result.stage_code,
                        "网关认证失败",
                        &auth_result.message,
                        &auth_result.suggestion,
                    );
                }

                let stage_tag = if let Some(ref diag) = self.last_diagnostic {
                    if !diag.stage_code.is_empty() && diag.stage_code != "OK" {
                        format!("[{}] ", diag.stage_code)
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                let reason_summary = if !auth_result.success && !auth_result.message.is_empty() {
                    format!("{}: ", auth_result.message)
                } else if auth_result.success {
                    "等待网关防火墙规则放行: ".to_string()
                } else {
                    String::new()
                };

                let user_friendly_msg = format!(
                    "{}{}{}秒后重试 (第{}次)",
                    stage_tag, reason_summary, backoff_secs, self.consecutive_failures
                );

                let clamped_msg = if user_friendly_msg.chars().count() > 110 {
                    let mut s: String = user_friendly_msg.chars().take(107).collect();
                    s.push_str("...");
                    s
                } else {
                    user_friendly_msg
                };

                self.transition_to(NetworkState::BackoffWait, clamped_msg);
            }
        }
    }

    /// 触发自动重连动作流水线（同步执行接口）
    pub fn trigger_reconnect(&mut self) {
        let detected_url = match self.can_prepare_reconnect() {
            Ok(url) => url,
            Err(_) => return,
        };

        let is_browser_mode = self.config.auth.mode == "browser";
        let allow_browser_open = self.should_allow_browser_open();

        let auth_result: AuthResult = AuthExecutor::execute(
            &self.config,
            detected_url.as_deref(),
            allow_browser_open,
        );

        if is_browser_mode && allow_browser_open && auth_result.success {
            std::thread::sleep(Duration::from_millis(1500));
        } else if auth_result.success {
            std::thread::sleep(Duration::from_millis(300));
        }

        let verify_report = self.probe.check_with_report();
        self.apply_reconnect_result(auth_result, verify_report);
    }

    /// 每秒滴答：驱动退避倒计时实时刷新与到期重试判断
    /// 返回值：(is_backoff_expired, should_quick_verify)
    pub fn tick_second(&mut self) -> (bool, bool) {
        let now = Instant::now();
        let is_browser = self.config.auth.mode == "browser"
            || self.last_browser_open_at.is_some();

        if let Some(until) = self.backoff_until {
            if now >= until {
                self.backoff_until = None;
                return (true, false);
            }
            let rem = until.duration_since(now).as_secs() + 1;
            let stage_tag = self.last_diagnostic.as_ref()
                .filter(|d| !d.stage_code.is_empty() && d.stage_code != "OK")
                .map(|d| format!("[{}] ", d.stage_code))
                .unwrap_or_default();

            if is_browser {
                if self.browser_open_count >= 3 {
                    self.last_message = format!("{}等待网页登录中... ({}秒后再次检测)", stage_tag, rem);
                } else {
                    self.last_message = format!("{}等待网页登录中，{}秒后再次检测", stage_tag, rem);
                }
            } else {
                let reason = self.last_diagnostic.as_ref()
                    .map(|d| format!("{}: ", d.title))
                    .unwrap_or_default();
                self.last_message = format!("{}{}{}秒后重试 (第{}次)", stage_tag, reason, rem, self.consecutive_failures);
            }

            let should_quick_verify = is_browser && (rem % 3 == 0);
            return (false, should_quick_verify);
        }

        (false, false)
    }

    /// 立即清空退避与错误计数
    pub fn clear_backoff(&mut self) {
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.last_browser_open_at = None;
        self.browser_open_count = 0;
    }

    /// 生成格式化专业排障诊断报告（供用户右键托盘查看）
    pub fn generate_diagnostic_report(&self) -> String {
        let snap = self.snapshot();
        let lan_ip = crate::config::get_lan_adapter_ipv4()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "未获取 (物理网卡未连接或无有效 IPv4)".to_string());
        let current_time = get_local_time_string();

        let diag = self.last_diagnostic.clone().unwrap_or_else(|| DiagnosticRecord {
            stage_code: "UNKNOWN".to_string(),
            title: "尚未进行网络诊断".to_string(),
            detail: "系统刚刚启动".to_string(),
            suggestion: "请等待首次网络探测完成".to_string(),
            timestamp: current_time.clone(),
        });

        let username = self
            .config
            .auth
            .http
            .params
            .get("userId")
            .or_else(|| self.config.auth.http.params.get("username"))
            .cloned()
            .unwrap_or_default();
        let masked_user = mask_username(&username);

        let password = self
            .config
            .auth
            .http
            .params
            .get("password")
            .cloned()
            .unwrap_or_default();
        let masked_pwd = mask_password(&password);

        let detected_url = self
            .last_detected_portal_url
            .as_deref()
            .unwrap_or("(未捕获到重定向 URL)");

        format!(
r#"================================================================================
                    NetTrigger 网络排障与状态诊断报告
================================================================================
生成时间: {}
程序版本: NetTrigger v{} (Release x86_64-pc-windows-msvc)
常驻时长: {} 秒

【一、当前网络整体状态】
  • 运行状态: {:?}
  • 连续失败: {} 次
  • 探针延迟: {} ms
  • 最新通知: {}

【二、近期诊断与错误阶段 (Diagnostic Stage)】
  • 诊断代码: [{}]
  • 问题诊断: {}
  • 详细信息: {}
  • 诊断时间: {}
  • 排障建议: {}

【三、底层网卡与链路信息】
  • 本地局域网 IPv4: {}
  • 探针检测端点: {}
  • 捕获认证重定向: {}

【四、当前认证配置参数 (已脱敏)】
  • 认证模式: {}
  • 登录网页网址: {}
  • 登录接口网址: {}
  • 认证学号/账号: {}
  • 认证密码状态: {}

================================================================================
提示：如需修改账号密码或切换为【自动打开网页登录】模式，请右键托盘图标点击【设置】。
================================================================================
"#,
            current_time,
            env!("CARGO_PKG_VERSION"),
            snap.uptime_sec,
            snap.state,
            snap.consecutive_failures,
            snap.latency_ms,
            snap.last_message,
            diag.stage_code,
            diag.title,
            diag.detail,
            diag.timestamp,
            diag.suggestion,
            lan_ip,
            &self.config.probe.primary_url,
            detected_url,
            self.config.auth.mode,
            if self.config.auth.portal_url.is_empty() { "(未设置)" } else { &self.config.auth.portal_url },
            if self.config.auth.http.action_url.is_empty() { "(未设置)" } else { &self.config.auth.http.action_url },
            masked_user,
            masked_pwd,
        )
    }

    /// 响应 Windows 内核网络变动事件（网卡重连/IP跳变）
    ///
    /// 核心特性：瞬时重置所有失败计数与退避计时，以最高优先级瞬时唤醒重新认证！
    pub fn on_network_changed(&mut self) -> NetworkState {
        self.consecutive_failures = 0;
        self.backoff_until = None;
        self.last_browser_open_at = None;
        self.browser_open_count = 0;
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
        self.browser_open_count = 0;
        self.last_message = "用户手动触发网络检测与重连".to_string();
        let state = self.step_probe();
        if state != NetworkState::Online {
            self.trigger_reconnect();
        }
        self.current_state
    }

    /// 状态跃迁辅助函数
    pub fn transition_to(&mut self, new_state: NetworkState, message: String) {
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

#[cfg(windows)]
fn get_local_time_string() -> String {
    unsafe {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut st);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
}

#[cfg(not(windows))]
fn get_local_time_string() -> String {
    "N/A".to_string()
}

pub fn mask_username(u: &str) -> String {
    let s = u.trim();
    if s.is_empty() {
        return "(未配置)".to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 4 {
        return "*".repeat(chars.len());
    }
    let prefix_len = (chars.len() / 2).min(4);
    let mut masked = String::new();
    for &c in &chars[..prefix_len] {
        masked.push(c);
    }
    masked.push_str(&"*".repeat(chars.len() - prefix_len));
    masked
}

pub fn mask_password(p: &str) -> String {
    let s = p.trim();
    if s.is_empty() {
        "(未配置)".to_string()
    } else {
        format!("已配置 [{} 位字符]", s.chars().count())
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

    #[test]
    fn test_credential_masking() {
        assert_eq!(mask_username("2023000001"), "2023******");
        assert_eq!(mask_username("admin"), "ad***");
        assert_eq!(mask_username(""), "(未配置)");

        assert_eq!(mask_password("123456"), "已配置 [6 位字符]");
        assert_eq!(mask_password(""), "(未配置)");
    }

    #[test]
    fn test_diagnostic_report_generation() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);
        fsm.record_diagnostic(
            "E5-01",
            "网关拒绝登录",
            "用户不存在或密码错误",
            "请核对学号与密码",
        );

        let report = fsm.generate_diagnostic_report();
        assert!(report.contains("NetTrigger 网络排障与状态诊断报告"));
        assert!(report.contains("[E5-01]"));
        assert!(report.contains("用户不存在或密码错误"));
        assert!(report.contains("请核对学号与密码"));
    }

    #[test]
    fn test_apply_reconnect_result_browser_fallback_no_e6_01() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);

        let fallback_auth = AuthResult::fail_with_stage(
            0,
            "静默登录未成功，已自动唤起浏览器登录页面".to_string(),
            "E5-FALLBACK-BROWSER",
            "已为您打开校园网登录页面，请在网页中完成登录。".to_string(),
        );

        let verify_offline = ProbeReport {
            status: ProbeStatus::Offline { reason: "test".to_string() },
            latency_ms: 10,
        };

        fsm.apply_reconnect_result(fallback_auth, verify_offline);

        // 绝不可误判为 E6-01
        if let Some(ref diag) = fsm.last_diagnostic {
            assert_ne!(diag.stage_code, "E6-01");
        }
        assert_eq!(fsm.state(), NetworkState::BackoffWait);
        // 唤起浏览器后进入防抖冷却窗口（首次 60 秒），杜绝每秒疯狂弹窗
        let snap = fsm.snapshot();
        assert!(snap.backoff_remaining_sec <= 60 && snap.backoff_remaining_sec >= 50);
        assert!(snap.last_message.contains("等待网页端登录完成"));
    }

    #[test]
    fn test_browser_cooldown_exponential_growth_and_cap() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);

        // 初始状态：允许打开
        assert!(fsm.should_allow_browser_open());

        // 模拟第 1 次唤起浏览器
        fsm.record_browser_opened();
        assert_eq!(fsm.browser_open_count, 1);
        assert_eq!(fsm.calculate_browser_backoff_secs(), 60);
        // 处于冷却期中，禁止连续再次弹窗
        assert!(!fsm.should_allow_browser_open());

        // 模拟第 2 次唤起浏览器
        fsm.record_browser_opened();
        assert_eq!(fsm.browser_open_count, 2);
        assert_eq!(fsm.calculate_browser_backoff_secs(), 180);
        assert!(!fsm.should_allow_browser_open());

        // 模拟第 3 次唤起浏览器
        fsm.record_browser_opened();
        assert_eq!(fsm.browser_open_count, 3);
        assert_eq!(fsm.calculate_browser_backoff_secs(), 300);
        // 达到 3 次上限，彻底停止自动弹窗
        assert!(!fsm.should_allow_browser_open());

        // 网络变动后自愈重置
        fsm.on_network_changed();
        assert_eq!(fsm.browser_open_count, 0);
        assert!(fsm.should_allow_browser_open());
    }

    #[test]
    fn test_tick_second_live_countdown() {
        let config = Config::default();
        let mut fsm = StateMachine::new(config);
        fsm.consecutive_failures = 1;
        fsm.backoff_until = Some(Instant::now() + Duration::from_secs(10));
        fsm.current_state = NetworkState::BackoffWait;

        let (expired, _) = fsm.tick_second();
        assert!(!expired);
        let snap = fsm.snapshot();
        assert!(snap.last_message.contains("秒后重试"));
    }
}
