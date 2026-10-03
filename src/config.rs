//! 配置管理与动态参数宏扩展模块
//!
//! 负责加载/生成带详细中文注释的 `config.toml`，并提供动态参数宏替换：
//! - `{username}`：配置的认证用户名
//! - `{password}`：配置的认证密码
//! - `{ip}`：当前连网本机的内网 IP
//! - `{mac}`：当前活动网卡 MAC 地址
//! - `{time}` / `{time_ms}`：当前 13 位毫秒级 Unix 时间戳
//! - `{time_s}`：当前 10 位秒级 Unix 时间戳

use crate::utils::fs::get_config_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::net::UdpSocket;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 标准配置模板（首次运行无 config.toml 时自动原子化写入）
pub const DEFAULT_CONFIG_TEMPLATE: &str = r#"# ==============================================================================
# NetTrigger 核心配置文件 (config.toml)
# 修改保存后，右键托盘图标点击【一键测试当前认证配置】即可验证！
# ==============================================================================

[general]
# 托盘中显示的应用名称
app_name = "NetTrigger"

# 正常网络畅通时的探针心跳间隔（单位：秒，建议 30 ~ 60，避免浪费系统资源与电量）
heartbeat_interval_sec = 45

# 是否开启 Windows 系统级网络变动监听（0 延迟即刻重连，强烈建议保持 true）
enable_zero_latency_watcher = true

# 静默免打扰模式：true 时即使重连成功也不弹出系统通知，打游戏推荐开启
silent_mode = true

# 连续失败时的最大退避等待时间（秒，防止频繁重发导致校园网封号或机房过载）
max_backoff_sec = 60


# ==============================================================================
# 网络连通性探针配置（通常保持默认即可，已针对国内网络与 Windows 深度优化）
# ==============================================================================
[probe]
# 首选探测地址（采用超低延迟的标准 204 无内容端点）
primary_url = "http://connect.rom.miui.com/generate_204"

# 备用探测地址（Windows 官方连通性测试端点）
fallback_url = "http://www.msftconnecttest.com/connecttest.txt"
fallback_expected_keyword = "Microsoft Connect Test"

# 单次探测超时时间（毫秒）
timeout_ms = 3000


# ==============================================================================
# 自动认证动作配置
# 模式选择：
#   "http"    - 【推荐】后台全静默登录，无需打开浏览器，完全无感（需配置下方 [auth.http]）
#   "browser" - 【极简】掉线时自动在系统浏览器打开登录网页，由浏览器记住密码自动登录
# ==============================================================================
[auth]
mode = "http"

# 校园网/公共网登录门户网页 URL（浏览器模式或右键菜单直接打开时使用）
portal_url = "http://portal.example.edu/"

# ------------------------------------------------------------------------------
# 静默 HTTP 认证配置（仅在 mode = "http" 时生效）
# 提示：在浏览器登录页面按 F12 -> Network(网络) -> 点击登录 -> 找到登录 POST/GET 请求
#       查看 Request URL 与 Payload 参数，填入下方即可！
# ------------------------------------------------------------------------------
[auth.http]
# 请求方法：POST 或 GET
method = "POST"

# 登录接口真实提交地址（非网页地址，通常带有 /login 或 /drcom 等后缀）
action_url = "http://portal.example.edu/api/login"

# 登录提交的表单参数（键值对形式）
# 支持智能动态宏变量：
#   {username} - 自动替换为上方用户配置
#   {password} - 自动替换为上方密码配置
#   {ip}       - 自动获取本机当前内网 IP
#   {mac}      - 自动获取本机 MAC 地址
#   {time}     - 自动填充当前 13 位毫秒时间戳
#   {time_s}   - 自动填充当前 10 位秒时间戳
[auth.http.params]
username = "your_student_id_or_username"
password = "your_password_here"
# 如果你的校园网需要选择运营商（例如: cmcc/unicom/telecom），可在此增加字段:
# domain = "telecom"

# HTTP 请求头（防被网关反爬校验拦截）
[auth.http.headers]
User-Agent = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
Content-Type = "application/x-www-form-urlencoded"
"#;

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(err) => write!(f, "配置文件 I/O 错误: {}", err),
            ConfigError::Parse(err) => write!(f, "配置文件 TOML 格式错误: {}", err),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(err: std::io::Error) -> Self {
        ConfigError::Io(err)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(err: toml::de::Error) -> Self {
        ConfigError::Parse(err)
    }
}

/// 核心全局配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,

    #[serde(default)]
    pub probe: ProbeConfig,

    #[serde(default)]
    pub auth: AuthConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            probe: ProbeConfig::default(),
            auth: AuthConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneralConfig {
    #[serde(default = "default_app_name")]
    pub app_name: String,

    #[serde(default = "default_heartbeat_interval_sec")]
    pub heartbeat_interval_sec: u64,

    #[serde(default = "default_true", alias = "enable_network_watcher")]
    pub enable_zero_latency_watcher: bool,

    #[serde(default = "default_true")]
    pub silent_mode: bool,

    #[serde(default = "default_max_backoff_sec")]
    pub max_backoff_sec: u64,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            app_name: default_app_name(),
            heartbeat_interval_sec: default_heartbeat_interval_sec(),
            enable_zero_latency_watcher: default_true(),
            silent_mode: default_true(),
            max_backoff_sec: default_max_backoff_sec(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeConfig {
    #[serde(default = "default_primary_url")]
    pub primary_url: String,

    #[serde(default = "default_fallback_url")]
    pub fallback_url: String,

    #[serde(default = "default_fallback_expected_keyword", alias = "fallback_expected_body")]
    pub fallback_expected_keyword: String,

    #[serde(default = "default_probe_timeout_ms", alias = "probe_timeout_ms")]
    pub timeout_ms: u64,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            primary_url: default_primary_url(),
            fallback_url: default_fallback_url(),
            fallback_expected_keyword: default_fallback_expected_keyword(),
            timeout_ms: default_probe_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthConfig {
    #[serde(default = "default_auth_mode")]
    pub mode: String,

    #[serde(default = "default_portal_url")]
    pub portal_url: String,

    #[serde(default)]
    pub http: HttpAuthConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: default_auth_mode(),
            portal_url: default_portal_url(),
            http: HttpAuthConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpAuthConfig {
    #[serde(default = "default_http_method")]
    pub method: String,

    #[serde(default = "default_action_url")]
    pub action_url: String,

    #[serde(default)]
    pub params: BTreeMap<String, String>,

    #[serde(default = "default_headers")]
    pub headers: BTreeMap<String, String>,
}

impl Default for HttpAuthConfig {
    fn default() -> Self {
        let mut params = BTreeMap::new();
        params.insert("username".to_string(), "your_student_id_or_username".to_string());
        params.insert("password".to_string(), "your_password_here".to_string());

        Self {
            method: default_http_method(),
            action_url: default_action_url(),
            params,
            headers: default_headers(),
        }
    }
}

// 默认值生成辅助函数
fn default_app_name() -> String {
    "NetTrigger".to_string()
}
fn default_heartbeat_interval_sec() -> u64 {
    45
}
fn default_true() -> bool {
    true
}
fn default_max_backoff_sec() -> u64 {
    60
}
fn default_primary_url() -> String {
    "http://connect.rom.miui.com/generate_204".to_string()
}
fn default_fallback_url() -> String {
    "http://www.msftconnecttest.com/connecttest.txt".to_string()
}
fn default_fallback_expected_keyword() -> String {
    "Microsoft Connect Test".to_string()
}
fn default_probe_timeout_ms() -> u64 {
    3000
}
fn default_auth_mode() -> String {
    "http".to_string()
}
fn default_portal_url() -> String {
    "http://portal.example.edu/".to_string()
}
fn default_http_method() -> String {
    "POST".to_string()
}
fn default_action_url() -> String {
    "http://portal.example.edu/api/login".to_string()
}
fn default_headers() -> BTreeMap<String, String> {
    let mut h = BTreeMap::new();
    h.insert(
        "User-Agent".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string(),
    );
    h.insert(
        "Content-Type".to_string(),
        "application/x-www-form-urlencoded".to_string(),
    );
    h
}

/// 动态参数宏执行上下文
#[derive(Debug, Clone, Default)]
pub struct MacroContext {
    pub username: String,
    pub password: String,
    pub ip: String,
    pub mac: String,
    pub time_ms: String,
    pub time_s: String,
}

impl MacroContext {
    /// 自动从系统环境及配置构建宏上下文
    pub fn build(config: &Config) -> Self {
        let username = config
            .auth
            .http
            .params
            .get("username")
            .cloned()
            .unwrap_or_default();
        let password = config
            .auth
            .http
            .params
            .get("password")
            .cloned()
            .unwrap_or_default();

        let (time_ms, time_s) = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_millis().to_string(), d.as_secs().to_string()),
            Err(_) => ("0".to_string(), "0".to_string()),
        };

        let ip = get_local_outbound_ip().unwrap_or_else(|| "127.0.0.1".to_string());
        let mac = get_local_mac_address().unwrap_or_else(|| "00:00:00:00:00:00".to_string());

        Self {
            username,
            password,
            ip,
            mac,
            time_ms,
            time_s,
        }
    }

    /// 对单个字符串文本执行宏变量展开
    pub fn expand(&self, input: &str) -> String {
        input
            .replace("{username}", &self.username)
            .replace("{password}", &self.password)
            .replace("{ip}", &self.ip)
            .replace("{mac}", &self.mac)
            .replace("{time}", &self.time_ms)
            .replace("{time_ms}", &self.time_ms)
            .replace("{time_s}", &self.time_s)
    }
}

/// 获取本机当前有效出接口的内网 IP 地址
///
/// 方案说明：通过轻量无开销的 UDP 伪连接（不产生任何网络流量），
/// 直接由内核路由表查询当前连网网卡的内网 IP。
pub fn get_local_outbound_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // 尝试国内常用公共 DNS 探测路由
    if socket.connect("223.5.5.5:80").is_ok() {
        if let Ok(addr) = socket.local_addr() {
            let ip = addr.ip().to_string();
            if ip != "0.0.0.0" {
                return Some(ip);
            }
        }
    }
    // 备用地址探测
    if socket.connect("119.29.29.29:80").is_ok() {
        if let Ok(addr) = socket.local_addr() {
            let ip = addr.ip().to_string();
            if ip != "0.0.0.0" {
                return Some(ip);
            }
        }
    }
    None
}

/// 获取本机主网卡的 MAC 地址（Windows 下安全提取，绝不崩溃）
pub fn get_local_mac_address() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::NetworkManagement::IpHelper::{
            GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
            GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
        };
        use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

        unsafe {
            let mut buf_len: u32 = 15000;
            let mut buffer: Vec<u8> = vec![0u8; buf_len as usize];

            let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
            let res = GetAdaptersAddresses(
                AF_UNSPEC as u32,
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut buf_len,
            );

            if res == 0 {
                let mut current = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
                while !current.is_null() {
                    let adapter = &*current;
                    // IfType 6 = MIB_IF_TYPE_ETHERNET, 71 = IF_TYPE_IEEE80211 (WiFi)
                    // OperStatus 1 = IfOperStatusUp
                    if (adapter.IfType == 6 || adapter.IfType == 71)
                        && adapter.OperStatus == 1
                        && adapter.PhysicalAddressLength == 6
                    {
                        let b = adapter.PhysicalAddress;
                        return Some(format!(
                            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                            b[0], b[1], b[2], b[3], b[4], b[5]
                        ));
                    }
                    current = adapter.Next;
                }
            }
        }
    }

    None
}

impl Config {
    /// 加载配置文件；若不存在，则原子化创建带中文注释的默认配置文件并返回
    pub fn load_or_create() -> Result<(Self, bool), ConfigError> {
        let path = get_config_path();
        if !path.exists() {
            // 确保目录存在
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            // 写入中文注释模板
            fs::write(&path, DEFAULT_CONFIG_TEMPLATE)?;
            let config: Config = toml::from_str(DEFAULT_CONFIG_TEMPLATE)?;
            return Ok((config, true));
        }

        let content = fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;
        Ok((config, false))
    }

    /// 从指定物理路径读取配置
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// 获取经过动态宏替换后的表单参数映射表
    pub fn get_expanded_params(&self) -> BTreeMap<String, String> {
        let ctx = MacroContext::build(self);
        self.get_expanded_params_with_ctx(&ctx)
    }

    /// 使用指定宏上下文进行展开（适用于单测或预置参数）
    pub fn get_expanded_params_with_ctx(&self, ctx: &MacroContext) -> BTreeMap<String, String> {
        let mut expanded = BTreeMap::new();
        for (k, v) in &self.auth.http.params {
            expanded.insert(k.clone(), ctx.expand(v));
        }
        expanded
    }

    /// 获取经过宏展开后的请求 Action URL
    pub fn get_expanded_action_url(&self, ctx: &MacroContext) -> String {
        ctx.expand(&self.auth.http.action_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_default_template() {
        let cfg_res: Result<Config, _> = toml::from_str(DEFAULT_CONFIG_TEMPLATE);
        assert!(cfg_res.is_ok(), "默认模板必须能够正常反序列化");
        if let Ok(cfg) = cfg_res {
            assert_eq!(cfg.general.app_name, "NetTrigger");
            assert_eq!(cfg.general.heartbeat_interval_sec, 45);
            assert!(cfg.general.enable_zero_latency_watcher);
            assert!(cfg.general.silent_mode);
            assert_eq!(cfg.probe.primary_url, "http://connect.rom.miui.com/generate_204");
            assert_eq!(cfg.auth.mode, "http");
            assert_eq!(cfg.auth.http.method, "POST");
        }
    }

    #[test]
    fn test_macro_expansion() {
        let ctx = MacroContext {
            username: "20230001".to_string(),
            password: "mypassword123".to_string(),
            ip: "10.10.1.50".to_string(),
            mac: "AA:BB:CC:DD:EE:FF".to_string(),
            time_ms: "1700000000123".to_string(),
            time_s: "1700000000".to_string(),
        };

        let raw = "user={username}&pass={password}&wlanuserip={ip}&mac={mac}&t={time}&ts={time_s}";
        let expanded = ctx.expand(raw);
        assert_eq!(
            expanded,
            "user=20230001&pass=mypassword123&wlanuserip=10.10.1.50&mac=AA:BB:CC:DD:EE:FF&t=1700000000123&ts=1700000000"
        );
    }

    #[test]
    fn test_config_param_macro_expansion() {
        let mut cfg = Config::default();
        cfg.auth.http.params.insert("userId".to_string(), "{username}".to_string());
        cfg.auth.http.params.insert("clientIp".to_string(), "{ip}".to_string());

        let ctx = MacroContext {
            username: "student88".to_string(),
            password: "p".to_string(),
            ip: "192.168.1.88".to_string(),
            mac: "".to_string(),
            time_ms: "123".to_string(),
            time_s: "1".to_string(),
        };

        let res = cfg.get_expanded_params_with_ctx(&ctx);
        assert_eq!(res.get("userId"), Some(&"student88".to_string()));
        assert_eq!(res.get("clientIp"), Some(&"192.168.1.88".to_string()));
    }
}
