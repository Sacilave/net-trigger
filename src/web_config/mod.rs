//! 按需瞬态微型 Web 配置服务 (Ephemeral Local Web Config)
//!
//! 核心设计：
//! 1. 纯标准库 `TcpListener` 实现，零大型 Web 框架依赖，平时 0% CPU、0 额外内存；
//! 2. 仅在用户从托盘点击“设置”时临时唤起，随机分配未占用端口；
//! 3. 原生调用默认浏览器打开内嵌的现代精致 Fluent / SVG 线性控制中心；
//! 4. 采用 serde_json 实现工业级参数序列化与反序列化，通过通道向主线程实时热同步；
//! 5. 空闲 5 分钟后自动销毁关闭 Socket，内存立即回归 1.5MB 纯净待机态。

use crate::auth::AuthResult;
use crate::config::Config;
use crate::utils::fs::get_config_path;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub const HTML_TEMPLATE: &str = include_str!("template.html");

static ACTIVE_SERVER: std::sync::Mutex<Option<(u16, Arc<AtomicBool>)>> = std::sync::Mutex::new(None);

fn open_browser_url(url: &str) {
    let url_string = url.to_string();
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            use windows_sys::Win32::System::Com::CoInitializeEx;

            unsafe {
                CoInitializeEx(std::ptr::null_mut(), 0x2 | 0x4);
            }

            let wide_op: Vec<u16> = "open\0".encode_utf16().collect();
            let wide_url: Vec<u16> = format!("{}\0", url_string).encode_utf16().collect();
            unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    wide_op.as_ptr(),
                    wide_url.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    SW_SHOWNORMAL as i32,
                );
            }
        }
    });
}

/// 启动瞬态 Web 配置服务，并在默认浏览器中唤起（支持活动实例复用）
pub fn launch_ephemeral_web_config(
    current_config: Config,
    on_config_saved: Option<Sender<Config>>,
    main_thread_id: u32,
) -> Result<u16, String> {
    // 检查既有服务是否还在运行，若存在直接复用既有端口唤起浏览器，避免重复绑定与开辟线程
    let mut lock = ACTIVE_SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((active_port, ref running_flag)) = *lock {
        if running_flag.load(Ordering::SeqCst) {
            let server_url = format!("http://127.0.0.1:{}/", active_port);
            open_browser_url(&server_url);
            return Ok(active_port);
        }
    }

    // 绑定回环随机端口
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("绑定本地临时端口失败: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("获取本地端口失败: {}", e))?
        .port();

    let server_url = format!("http://127.0.0.1:{}/", port);

    let is_running = Arc::new(AtomicBool::new(true));
    let is_running_clone = Arc::clone(&is_running);
    *lock = Some((port, Arc::clone(&is_running)));

    // 在临时独立线程中运行微型 HTTP 服务 (先启动监听再唤起浏览器，保证浏览器一访问即刻响应)
    thread::Builder::new()
        .name("NetTrigger-WebConfig".to_string())
        .spawn(move || {
            let _ = listener.set_nonblocking(true);
            let timeout = Duration::from_secs(300); // 5分钟无任何请求自动安全释放 Socket 回归 1.5MB
            let mut last_active = std::time::Instant::now();
            let mut cached_config = current_config;

            while is_running_clone.load(Ordering::SeqCst) && last_active.elapsed() < timeout {
                match listener.accept() {
                    Ok((stream, _)) => {
                        last_active = std::time::Instant::now(); // 收到任何请求即刻刷新活跃时间
                        handle_http_client(stream, &mut cached_config, on_config_saved.as_ref(), main_thread_id);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20)); // 极短防抖轮询，兼顾 0% CPU 与 20ms 极速响应
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            // 自动标记退出，Socket 随 scope 彻底丢弃释放，并回收临时 Web 服务占用的工作集内存
            is_running_clone.store(false, Ordering::SeqCst);
            crate::utils::mem::trim_working_set();
        })
        .map_err(|e| format!("启动 Web 临时线程失败: {}", e))?;

    // 服务就绪后并发唤起浏览器
    open_browser_url(&server_url);

    Ok(port)
}

fn handle_http_client(
    mut stream: TcpStream,
    cached_config: &mut Config,
    on_save_tx: Option<&Sender<Config>>,
    main_thread_id: u32,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let mut buffer = [0u8; 8192];
    let n = match stream.read(&mut buffer) {
        Ok(bytes) if bytes > 0 => bytes,
        _ => return,
    };

    let request_str = String::from_utf8_lossy(&buffer[..n]);
    let mut lines = request_str.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return,
    };

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }

    let method = parts[0];
    let path = parts[1];

    if method == "GET" && path == "/" {
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            HTML_TEMPLATE.len(),
            HTML_TEMPLATE
        );
        let _ = stream.write_all(resp.as_bytes());
    } else if method == "GET" && path == "/api/config" {
        let mut json_val = serde_json::to_value(&*cached_config)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(ref mut map) = json_val {
            map.insert(
                "effective_language".to_string(),
                serde_json::Value::String(cached_config.general.effective_language().to_string()),
            );
        }
        let json_res = serde_json::to_string(&json_val)
            .unwrap_or_else(|_| toml_to_json_str(cached_config));
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            json_res.len(),
            json_res
        );
        let _ = stream.write_all(resp.as_bytes());
    } else if method == "POST" && path == "/api/save" {
        if let Some(body) = extract_http_body(&request_str) {
            let parse_result: Result<Config, _> = serde_json::from_str(body)
                .or_else(|_| parse_json_to_config(body, cached_config));

            if let Ok(new_cfg) = parse_result {
                *cached_config = new_cfg.clone();
                // 1. 格式化序列化回 toml 并持久化保存到磁盘
                if let Ok(toml_str) = toml::to_string_pretty(&new_cfg) {
                    let _ = fs::write(get_config_path(), toml_str);
                }
                // 2. 核心通道同步：通知主线程状态机即刻热更新！
                if let Some(tx) = on_save_tx {
                    let _ = tx.send(new_cfg);
                }
                #[cfg(windows)]
                if main_thread_id != 0 {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};
                    unsafe {
                        PostThreadMessageW(main_thread_id, WM_APP, 0, 0);
                    }
                }

                let resp_body = "{\"success\":true}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
        }
        let resp_body = "{\"success\":false,\"message\":\"参数解析失败\"}";
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        let _ = stream.write_all(resp.as_bytes());
    } else if method == "POST" && path == "/api/open_config_file" {
        let config_file = get_config_path();
        let _ = std::process::Command::new("notepad.exe")
            .arg(config_file)
            .spawn();
        let resp_body = "{\"success\":true}";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        let _ = stream.write_all(resp.as_bytes());
    } else if method == "POST" && path == "/api/test" {
        if let Some(body) = extract_http_body(&request_str) {
            let parse_result: Result<Config, _> = serde_json::from_str(body)
                .or_else(|_| parse_json_to_config(body, cached_config));

            if let Ok(temp_cfg) = parse_result {
                let auth_res: AuthResult = crate::auth::AuthExecutor::execute(&temp_cfg);
                let result_json = format!(
                    "{{\"success\":{},\"status_code\":{},\"message\":\"{}\"}}",
                    auth_res.success,
                    auth_res.status_code,
                    escape_json_str(&auth_res.message)
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    result_json.len(),
                    result_json
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
        }
        let resp_body = "{\"success\":false,\"status_code\":0,\"message\":\"无法解析测试数据\"}";
        let resp = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            resp_body.len(),
            resp_body
        );
        let _ = stream.write_all(resp.as_bytes());
    } else {
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
    }
}

fn extract_http_body(req: &str) -> Option<&str> {
    if let Some(idx) = req.find("\r\n\r\n") {
        Some(&req[idx + 4..])
    } else {
        None
    }
}

fn escape_json_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', "")
}

/// 快速将 Config 转换为简易 JSON 返回给前端 (兼容兜底)
fn toml_to_json_str(config: &Config) -> String {
    let mut params_parts = Vec::new();
    for (k, v) in &config.auth.http.params {
        params_parts.push(format!("\"{}\":\"{}\"", escape_json_str(k), escape_json_str(v)));
    }

    format!(
        r#"{{"general":{{"profile":"{}","silent_mode":{},"allow_browser_fallback":{},"language":"{}"}},"effective_language":"{}","auth":{{"mode":"{}","portal_url":"{}","http":{{"action_url":"{}","method":"{}","params":{{{}}}}}}}}}"#,
        config.general.profile,
        config.general.silent_mode,
        config.general.allow_browser_fallback,
        config.general.language,
        config.general.effective_language(),
        config.auth.mode,
        escape_json_str(&config.auth.portal_url),
        escape_json_str(&config.auth.http.action_url),
        config.auth.http.method,
        params_parts.join(",")
    )
}

/// 将前端提交的简单 JSON 解析更新到 Config (轻量回退处理)
fn parse_json_to_config(json: &str, fallback: &Config) -> Result<Config, ()> {
    let mut updated = fallback.clone();

    if let Some(mode) = extract_json_value(json, "mode") {
        updated.auth.mode = mode;
    }
    if let Some(profile) = extract_json_value(json, "profile") {
        updated.general.profile = profile;
    }
    if let Some(lang) = extract_json_value(json, "language") {
        updated.general.language = match lang.to_lowercase().as_str() {
            "zh" => crate::config::Language::Zh,
            "en" => crate::config::Language::En,
            _ => crate::config::Language::Auto,
        };
    }
    if let Some(silent) = extract_json_bool(json, "silent_mode") {
        updated.general.silent_mode = silent;
    }
    if let Some(allow_fallback) = extract_json_bool(json, "allow_browser_fallback") {
        updated.general.allow_browser_fallback = allow_fallback;
    }
    if let Some(portal) = extract_json_value(json, "portal_url") {
        updated.auth.portal_url = portal;
    }
    if let Some(action) = extract_json_value(json, "action_url") {
        updated.auth.http.action_url = action;
    }
    if let Some(method) = extract_json_value(json, "method") {
        updated.auth.http.method = method;
    }

    Ok(updated)
}

fn extract_json_value(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

fn extract_json_bool(json: &str, key: &str) -> Option<bool> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = json[start..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_conversion() {
        let mut cfg = Config::default();
        cfg.auth.mode = "browser".to_string();
        let json = toml_to_json_str(&cfg);
        assert!(json.contains("\"profile\":\"gaming\""));
        assert!(json.contains("\"mode\":\"browser\""));
        assert!(json.contains("\"language\":\"auto\""));

        let parsed = parse_json_to_config(&json, &cfg).unwrap();
        assert_eq!(parsed.general.profile, "gaming");
        assert_eq!(parsed.auth.mode, "browser");
        assert_eq!(parsed.general.language, crate::config::Language::Auto);
    }
}
