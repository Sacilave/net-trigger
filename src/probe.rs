//! Captive Portal 权威探测模块
//!
//! 核心设计哲学：
//! 1. 显式禁用 HTTP 重定向（FollowRedirects = false），捕获 301/302/307 劫持；
//! 2. 优先采用标准 204 No Content 端点，零冗余 Body 下载；
//! 3. 两级探针融合仲裁（Primary 204 + Fallback 微软 NCSI），杜绝单点误判与假在线；
//! 4. 紧凑超时控制与内存严格约束，杜绝 panic 与请求风暴。

use crate::config::ProbeConfig;
use std::io::Read;
use std::time::{Duration, Instant};

/// 网络探测判决状态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeStatus {
    /// 互联网完全畅通 (204 成功或 Fallback 端点内容严格匹配)
    Online,
    /// 检测到 Captive Portal 认证拦截 (收到 30x 重定向或认证网页劫持)
    CaptivePortal {
        /// 重定向的目标认证 URL（若网关在 Location 头中提供了）
        redirect_url: Option<String>,
    },
    /// 网络物理断开、DNS 失败或路由不可达
    Offline {
        /// 离线或不可达具体原因
        reason: String,
    },
}

impl ProbeStatus {
    pub fn is_online(&self) -> bool {
        matches!(self, ProbeStatus::Online)
    }

    pub fn is_captive_portal(&self) -> bool {
        matches!(self, ProbeStatus::CaptivePortal { .. })
    }

    pub fn is_offline(&self) -> bool {
        matches!(self, ProbeStatus::Offline { .. })
    }
}

/// 探测结果综合报告（包含延迟指标，供托盘 Tooltip 与日志展示）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub status: ProbeStatus,
    /// 单次探测网络往返耗时 (毫秒)
    pub latency_ms: u64,
}

/// 权威探针执行器
#[derive(Clone)]
pub struct Probe {
    agent: ureq::Agent,
    primary_url: String,
    fallback_url: String,
    fallback_expected_keyword: String,
    portal_url: Option<String>,
}

impl Probe {
    /// 基于配置创建探针实例
    pub fn new(config: &ProbeConfig) -> Self {
        Self::with_portal(config, None)
    }

    /// 基于探针配置与可选认证网关网址创建探针实例
    pub fn with_portal(config: &ProbeConfig, portal_url: Option<&str>) -> Self {
        let timeout_ms = if config.timeout_ms == 0 {
            3000
        } else {
            config.timeout_ms
        };

        // 紧凑超时与显式禁用重定向
        let connect_timeout = Duration::from_millis(timeout_ms.min(2000));
        let read_timeout = Duration::from_millis(timeout_ms);

        let agent = ureq::builder()
            .redirects(0) // 严格禁用重定向！杜绝 302 假在线
            .timeout_connect(connect_timeout)
            .timeout_read(read_timeout)
            .user_agent("NetTrigger-Probe/1.0 (Windows NT 10.0; Win64; x64)")
            .build();

        Self {
            agent,
            primary_url: config.primary_url.clone(),
            fallback_url: config.fallback_url.clone(),
            fallback_expected_keyword: config.fallback_expected_keyword.clone(),
            portal_url: portal_url.map(|s| s.trim().to_string()),
        }
    }

    /// 执行权威网络探测，返回状态判定
    pub fn check(&self) -> ProbeStatus {
        self.check_with_report().status
    }

    /// 执行权威网络探测，并返回包含延迟（毫秒）的详细报告
    pub fn check_with_report(&self) -> ProbeReport {
        let start = Instant::now();

        // 1. 首选探测主端点 (通常为标准 204 端点)
        match self.probe_endpoint(&self.primary_url, true) {
            Ok(status) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                ProbeReport {
                    status,
                    latency_ms,
                }
            }
            Err(primary_err) => {
                // 主端点请求报错 (如超时或网络不可达)，触发备用端点仲裁
                match self.probe_endpoint(&self.fallback_url, false) {
                    Ok(fallback_status) => {
                        let latency_ms = start.elapsed().as_millis() as u64;
                        ProbeReport {
                            status: fallback_status,
                            latency_ms,
                        }
                    }
                    Err(fallback_err) => {
                        // 2. 主探针与备用探针均失败 (校园网未认证时 DNS 阻断外网域名属于常态)
                        // 此时不可直接武断判定为 Offline！执行三级权威降级仲裁：

                        // 2.1 探测已配置的认证网关端点
                        if let Some(ref portal) = self.portal_url {
                            let clean_portal = portal.trim();
                            if !clean_portal.is_empty() && !clean_portal.contains("example.") {
                                if let Ok(portal_status) = self.probe_portal_endpoint(clean_portal) {
                                    let latency_ms = start.elapsed().as_millis() as u64;
                                    return ProbeReport {
                                        status: portal_status,
                                        latency_ms,
                                    };
                                }
                            }
                        }

                        // 2.2 纯 IP 直连外网探测 (彻底规避 DNS 阻断，捕获校园网 AC 硬件对 80 端口的 302 劫持与完整动态重定向 URL)
                        for ip_target in &["http://1.1.1.1/", "http://123.123.123.123/"] {
                            if let Ok(ip_status) = self.probe_endpoint(ip_target, true) {
                                let latency_ms = start.elapsed().as_millis() as u64;
                                return ProbeReport {
                                    status: ip_status,
                                    latency_ms,
                                };
                            }
                        }

                        // 2.3 物理链路与内网分配判定：
                        // 严格核验是否连接了物理局域网网卡 (WiFi 或 以太网)！
                        // 严禁将移动蜂窝网络 (4G/5G LTE) 的 CGNAT 10.x.x.x 私网 IP 误判为校园网 Captive Portal！
                        if let Some(lan_ip) = crate::config::get_lan_adapter_ipv4() {
                            if is_lan_ip(&lan_ip) {
                                let latency_ms = start.elapsed().as_millis() as u64;
                                return ProbeReport {
                                    status: ProbeStatus::CaptivePortal {
                                        redirect_url: self.portal_url.clone(),
                                    },
                                    latency_ms,
                                };
                            }
                        }

                        let latency_ms = start.elapsed().as_millis() as u64;
                        ProbeReport {
                            status: ProbeStatus::Offline {
                                reason: format!("主探针: {}; 备用探针: {}", primary_err, fallback_err),
                            },
                            latency_ms,
                        }
                    }
                }
            }
        }
    }

    /// 探测指定端点
    ///
    /// - `is_204_endpoint`: 是否为预期返回 204 No Content 的端点
    fn probe_endpoint(&self, url: &str, is_204_endpoint: bool) -> Result<ProbeStatus, String> {
        let res = self.agent.get(url).call();

        match res {
            Ok(response) => {
                let status = response.status();

                // 核心裁决 1：HTTP 30x 重定向拦截（Captive Portal 权威判定）
                if (300..=399).contains(&status) {
                    let location = response
                        .header("Location")
                        .or_else(|| response.header("location"))
                        .map(|s| s.trim().to_string());
                    return Ok(ProbeStatus::CaptivePortal {
                        redirect_url: location,
                    });
                }

                if is_204_endpoint {
                    if status == 204 {
                        // 标准 204 No Content，判定在线
                        return Ok(ProbeStatus::Online);
                    } else if status == 200 {
                        // 204 端点却返回了 200 OK，说明被网关劫持并返回了认证页面
                        // 限制最多读取 1KB 内容进行启发式特征检测
                        let mut buf = [0u8; 1024];
                        let mut reader = response.into_reader().take(1024);
                        let n = reader.read(&mut buf).unwrap_or(0);
                        let snippet = String::from_utf8_lossy(&buf[..n]).to_lowercase();

                        if snippet.contains("<html")
                            || snippet.contains("<script")
                            || snippet.contains("portal")
                            || snippet.contains("login")
                            || snippet.contains("radius")
                            || snippet.contains("drcom")
                        {
                            return Ok(ProbeStatus::CaptivePortal { redirect_url: None });
                        }

                        // 若没有明显特征但非 204，仍属异常响应，判为 CaptivePortal 拦截
                        return Ok(ProbeStatus::CaptivePortal { redirect_url: None });
                    } else {
                        // 其它 2xx 响应
                        return Ok(ProbeStatus::Online);
                    }
                } else {
                    // Fallback 端点 (例如 connecttest.txt 预期返回 200 及指定文本)
                    if status == 200 {
                        let mut body = String::new();
                        let mut reader = response.into_reader().take(1024);
                        if reader.read_to_string(&mut body).is_ok() {
                            if body.contains(&self.fallback_expected_keyword) {
                                return Ok(ProbeStatus::Online);
                            }
                        }
                        // 返回了 200 但正文不匹配，判定为网关替换劫持
                        return Ok(ProbeStatus::CaptivePortal { redirect_url: None });
                    }
                    Err(format!("备用端点返回非预期状态码: {}", status))
                }
            }
            Err(ureq::Error::Status(code, response)) => {
                // 301, 302, 303, 307, 308 重定向 -> 核心 Captive Portal 拦截判定
                if (300..=399).contains(&code) {
                    let location = response
                        .header("Location")
                        .or_else(|| response.header("location"))
                        .map(|s| s.trim().to_string());
                    return Ok(ProbeStatus::CaptivePortal {
                        redirect_url: location,
                    });
                }
                // 4xx / 5xx 服务器错误，作为端点故障上报
                Err(format!("端点响应异常状态码: {}", code))
            }
            Err(ureq::Error::Transport(transport_err)) => {
                // 网络链路错误（DNS失败、超时、连接重置、无可用网卡）
                Err(transport_err.to_string())
            }
        }
    }

    /// 探测配置的认证网关端点
    fn probe_portal_endpoint(&self, portal_url: &str) -> Result<ProbeStatus, String> {
        // 如果 portal_url 包含了 success.jsp，提取其 origin/base 或根路径，避免触发“原ip与当前用户不一致”
        let probe_target = if portal_url.contains("success.jsp") {
            if let Some(pos) = portal_url.find("/eportal/") {
                format!("{}/eportal/", &portal_url[..pos])
            } else if let Some(pos) = portal_url.find("/success.jsp") {
                format!("{}/", &portal_url[..pos])
            } else {
                portal_url.to_string()
            }
        } else {
            portal_url.to_string()
        };

        let res = self.agent.get(&probe_target).call();
        match res {
            Ok(response) => {
                let status = response.status();
                if (300..=399).contains(&status) {
                    let location = response
                        .header("Location")
                        .or_else(|| response.header("location"))
                        .map(|s| s.trim().to_string());
                    return Ok(ProbeStatus::CaptivePortal {
                        redirect_url: location,
                    });
                }
                // 200 OK: 校园网认证网关响应了页面
                Ok(ProbeStatus::CaptivePortal {
                    redirect_url: Some(probe_target),
                })
            }
            Err(ureq::Error::Status(code, response)) => {
                if (300..=399).contains(&code) {
                    let location = response
                        .header("Location")
                        .or_else(|| response.header("location"))
                        .map(|s| s.trim().to_string());
                    return Ok(ProbeStatus::CaptivePortal {
                        redirect_url: location,
                    });
                }
                // 哪怕返回了 401/403/404 等，也说明网关服务活着，属于 CaptivePortal 环境
                Ok(ProbeStatus::CaptivePortal {
                    redirect_url: Some(probe_target),
                })
            }
            Err(ureq::Error::Transport(err)) => Err(err.to_string()),
        }
    }
}

/// 判定 IP 地址是否属于私网局域网（10.x, 172.16-31.x, 192.168.x）且排除了 APIPA (169.254.x) 与 Loopback
fn is_lan_ip(ip_str: &str) -> bool {
    if let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() {
        let octets = ip.octets();
        if octets[0] == 127 || (octets[0] == 169 && octets[1] == 254) || octets[0] == 0 {
            return false;
        }
        if octets[0] == 10 {
            return true;
        }
        if octets[0] == 172 && (16..=31).contains(&octets[1]) {
            return true;
        }
        if octets[0] == 192 && octets[1] == 168 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_probe_status_predicates() {
        let online = ProbeStatus::Online;
        assert!(online.is_online());
        assert!(!online.is_captive_portal());
        assert!(!online.is_offline());

        let portal = ProbeStatus::CaptivePortal {
            redirect_url: Some("http://10.0.0.1/login".to_string()),
        };
        assert!(!portal.is_online());
        assert!(portal.is_captive_portal());
        assert!(!portal.is_offline());

        let offline = ProbeStatus::Offline {
            reason: "DNS timeout".to_string(),
        };
        assert!(!offline.is_online());
        assert!(!offline.is_captive_portal());
        assert!(offline.is_offline());
    }

    #[test]
    fn test_probe_initialization() {
        let config = ProbeConfig {
            primary_url: "http://connect.rom.miui.com/generate_204".to_string(),
            fallback_url: "http://www.msftconnecttest.com/connecttest.txt".to_string(),
            fallback_expected_keyword: "Microsoft Connect Test".to_string(),
            timeout_ms: 2000,
        };

        let probe = Probe::new(&config);
        assert_eq!(probe.primary_url, "http://connect.rom.miui.com/generate_204");
        assert_eq!(probe.fallback_url, "http://www.msftconnecttest.com/connecttest.txt");
    }

    fn serve_mock_response(listener: std::net::TcpListener, response: &'static [u8]) {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line) {
                    if n == 0 || line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let _ = stream.write_all(response);
                let _ = stream.flush();
            }
        });
    }

    #[test]
    fn test_probe_204_returns_online() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local_addr").port();

        serve_mock_response(
            listener,
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );

        let config = ProbeConfig {
            primary_url: format!("http://127.0.0.1:{}/generate_204", port),
            fallback_url: "http://127.0.0.1:1/unreachable".to_string(),
            fallback_expected_keyword: "unused".to_string(),
            timeout_ms: 2000,
        };

        let probe = Probe::new(&config);
        let status = probe.check();
        assert_eq!(status, ProbeStatus::Online);
    }

    #[test]
    fn test_probe_302_redirect_returns_captive_portal() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local_addr").port();

        serve_mock_response(
            listener,
            b"HTTP/1.1 302 Found\r\nLocation: http://portal.campus.edu/login.html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );

        let config = ProbeConfig {
            primary_url: format!("http://127.0.0.1:{}/generate_204", port),
            fallback_url: "http://127.0.0.1:1/unreachable".to_string(),
            fallback_expected_keyword: "unused".to_string(),
            timeout_ms: 2000,
        };

        let probe = Probe::new(&config);
        let status = probe.check();
        assert_eq!(
            status,
            ProbeStatus::CaptivePortal {
                redirect_url: Some("http://portal.campus.edu/login.html".to_string())
            }
        );
    }

    #[test]
    fn test_probe_200_html_intercept_returns_captive_portal() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().expect("local_addr").port();

        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 72\r\nConnection: close\r\n\r\n<html><script>window.location='http://portal.campus.edu'</script></html>";
        serve_mock_response(listener, response);

        let config = ProbeConfig {
            primary_url: format!("http://127.0.0.1:{}/generate_204", port),
            fallback_url: "http://127.0.0.1:1/unreachable".to_string(),
            fallback_expected_keyword: "unused".to_string(),
            timeout_ms: 2000,
        };

        let probe = Probe::new(&config);
        let status = probe.check();
        assert_eq!(
            status,
            ProbeStatus::CaptivePortal {
                redirect_url: None
            }
        );
    }

    #[test]
    fn test_probe_fallback_arbitration() {
        use std::net::TcpListener;

        let fallback_listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let fallback_port = fallback_listener.local_addr().expect("local_addr").port();

        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 22\r\nConnection: close\r\n\r\nMicrosoft Connect Test";
        serve_mock_response(fallback_listener, response);

        let config = ProbeConfig {
            // 主端点故意指向一个未监听端口，触发 Transport 异常
            primary_url: "http://127.0.0.1:54321/generate_204".to_string(),
            fallback_url: format!("http://127.0.0.1:{}/connecttest.txt", fallback_port),
            fallback_expected_keyword: "Microsoft Connect Test".to_string(),
            timeout_ms: 500,
        };

        let probe = Probe::new(&config);
        let status = probe.check();
        assert_eq!(status, ProbeStatus::Online);
    }

    #[test]
    fn test_probe_portal_fallback_when_primary_and_fallback_unreachable() {
        use std::net::TcpListener;

        let portal_listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let portal_port = portal_listener.local_addr().expect("local_addr").port();

        serve_mock_response(
            portal_listener,
            b"HTTP/1.1 302 Found\r\nLocation: http://10.10.200.102/eportal/index.jsp?wlanuserip=1.2.3.4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );

        let config = ProbeConfig {
            primary_url: "http://127.0.0.1:54321/unreachable_204".to_string(),
            fallback_url: "http://127.0.0.1:54322/unreachable_txt".to_string(),
            fallback_expected_keyword: "unused".to_string(),
            timeout_ms: 500,
        };

        let portal_url = format!("http://127.0.0.1:{}/", portal_port);
        let probe = Probe::with_portal(&config, Some(&portal_url));
        let status = probe.check();
        assert_eq!(
            status,
            ProbeStatus::CaptivePortal {
                redirect_url: Some("http://10.10.200.102/eportal/index.jsp?wlanuserip=1.2.3.4".to_string())
            }
        );
    }
}
