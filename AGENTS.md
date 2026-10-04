# AGENTS.md — NetTrigger 架构治理与 Agent 协同开发准则

> **适用对象**：所有参与 NetTrigger 项目设计、开发、重构、审查与维护的 AI 编程助手（Agents）与人类开发者。  
> **核心使命**：确保 NetTrigger 在全生命周期中始终坚守“极致轻量、零延迟事件响应、无感后台守护、工业级防崩”的核心设计哲学。

---

## 1. 项目定位与核心哲学 (Philosophy)

NetTrigger 不是一个传统的简单定时轮询脚本，而是一个**专为 Windows 平台打造的原生、超轻量（RAM < 3MB）、基于事件驱动（近乎 0 延迟）的网络状态感知与动作自动化响应引擎**。

### 核心设计守则 (Non-Negotiable Principles)
1. **内存与体积红线**：
   - 运行时常驻内存（Working Set）严格保持在 **3MB 以内**（理想状态 1.5MB ~ 2MB）。
   - Release 编译产物必须单文件免安装，经 LTO 和 Strip 处理后体积控制在 **3MB 以内**。
   - 严禁引入任何重量级运行时（如 Web 容器、Electron、Chromium、重型 GUI 框架）。
2. **事件驱动优先于盲目轮询**：
   - 严禁采用 1~2 秒高频死循环轮询！这是电池消耗和无线信道浪费的元凶。
   - 必须通过 Windows 原生网络通知（IP Helper API / `NotifyAddrChange` / 事件通知机制）实现“掉线/连网即刻唤醒”；
   - 心跳保活探测仅作为兜底机制（默认 30s ~ 60s 一次）。
3. **零假在线（Captive Portal 权威探测）**：
   - 严禁使用普通 HTTP 客户端跟随重定向（FollowRedirects 必须显式为 `false`）。
   - 必须利用标准 204 端点或特征响应精准辨别“真连通”与“校园网 302 劫持”，杜绝假在线。
4. **零打扰与防夺焦（Zero-Focus Stealing & Silent by Default）**：
   - 核心执行链路必须是后台纯静默 HTTP POST/GET 认证；
   - 严禁在未经用户允许的情况下强行弹出外部浏览器窗口抢占屏幕焦点（打游戏、全屏观影或远程会议时 100% 保持前台焦点不切屏、不掉帧）；
   - `silent_mode` 开启时，网络认证成功绝不发送任何桌面气泡通知。
5. **瞬时响应与黄金防抖（Instant Reconnect with Debounce）**：
   - 必须通过 Windows 原生网络通知实现毫秒级响应；
   - 采用 50ms ~ 150ms 黄金链路稳定防抖窗口（Link Stabilization Debounce），杜绝物理层刚建立但本地路由与 DHCP 尚未完全生效时的无效发包与请求风暴。
6. **场景化预设与拒绝过度复杂化（Profile Presets）**：
   - 提供 `gaming` (50ms防抖/15s心跳/强静默)、`balanced` (150ms防抖/45s心跳)、`power_save` (500ms防抖/90s心跳) 档位；
   - 保持底层 CPU 0.00%、内存 < 2.0MB 的绝对红线，不让用户在无意义的繁琐底层参数上做选择题。
7. **工业级鲁棒性（Crash-Proof）**：
   - 作为长期常驻后台的守护程序，**严禁在任何业务流程中使用未受保护的 `unwrap()` 或 `expect()`**。
   - 任何网络超时、DNS 解析失败、配置文件格式错误、网卡断开等，都必须作为正常的状态机输入流安全消化。

---

## 2. 系统模块划分与架构边界 (Module Boundaries)

NetTrigger 采用清晰的分层解耦架构，严禁出现上帝对象（God Objects）：

```
NetTrigger/
├── docs/                      # 架构与需求设计详档
│   └── REQUIREMENTS_AND_ARCHITECTURE.md
├── src/
│   ├── main.rs                # 进程入口、单实例互斥锁、顶层事件循环
│   ├── config.rs              # TOML 配置解析、默认值初始化、基准路径计算
│   ├── state.rs               # 核心有限状态机 (FSM) 与状态迁移事件
│   ├── probe.rs               # Captive Portal 权威探针 (禁用重定向, 204判定)
│   ├── auth/                  # 认证执行管道 (Action Pipeline)
│   │   ├── mod.rs             # 动作执行调度器
│   │   ├── http_client.rs     # 静默 HTTP POST/GET 模拟登录
│   │   └── browser.rs         # Fallback 浏览器唤起 (ShellExecuteW)
│   ├── watcher/               # 操作系统事件驱动模块
│   │   ├── mod.rs
│   │   └── win_network.rs     # Windows 网络状态监听 (NotifyAddrChange / IP Helper)
│   ├── tray/                  # 极轻量 Windows 托盘交互
│   │   ├── mod.rs             # 托盘图标状态切换 (🟢 在线 / 🟡 重连 / 🔴 离线)
│   │   └── autostart.rs       # HKCU\Run 注册表开机自启安全管理
│   └── utils/
│       ├── fs.rs              # 安全基准路径解析 (基于 current_exe)
│       └── logger.rs          # 极简轻量滚动日志 (可选控制台/文件)
├── Cargo.toml                 # 依赖声明与编译体积极致优化配置
├── AGENTS.md                  # 本规范文档
└── README.md                  # 用户使用说明
```

### 各模块职责定义
- **`state.rs` (State Machine)**：系统的中枢大脑。维护状态（`Online`, `CaptivePortal`, `Authenticating`, `Disconnected`, `Cooldown`），只接受来自 `probe`、`watcher` 和 `ui` 的消息并执行确定性的状态跃迁。
- **`probe.rs` (Network Probe)**：只负责发起网络探测并返回精确的连通判定（`Online` / `PortalIntercepted(location)` / `Offline(error)`）。不直接触发登录动作。
- **`auth/` (Action Executor)**：接收来自状态机的重连指令，根据配置执行 HTTP 认证或唤起浏览器。
- **`watcher/` (System Event Watcher)**：在后台轻量监听操作系统层面的网卡状态变动，发出即时唤醒信号。
- **`tray/` (Tray & UI)**：轻量托盘菜单，渲染状态图标，接收用户菜单点击指令并桥接给状态机。

---

## 3. 核心机制实现指南 (Implementation Rules)

### 3.1 路径解析规范
```rust
// 严禁使用相对路径，例如 File::open("config.toml")
// 必须始终基于当前可执行文件所在的绝对路径：
pub fn get_app_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}
```

### 3.2 单实例互斥保证
```rust
// main.rs 中必须创建 Windows 命名互斥体，防止多开造成冲突：
// 互斥体标识符: L"Global\\NetTrigger_SingleInstance_Mutex"
```

### 3.3 探测器 HTTP 客户端配置
- 必须设置独立且紧凑的超时时间（例如 Connect Timeout: 2s, Read Timeout: 3s）。
- **必须设置 `redirect: false`**。
- 若接收到状态码 `302/301/307`，提取 `Location` 头部，标记为 `CaptivePortal(redirect_url)`。
- 若状态码为 `204`，标记为 `Online`。

### 3.4 指数退避与熔断策略
- 认证连续失败时：重试间隔按 `1s -> 2s -> 4s -> 8s -> 16s -> 30s -> 60s (上限)` 递增。
- 一旦探测恢复 `Online`，退避计时器与失败计数器立即归零重置。
- 达到最大退避时间（60s）后，系统挂起高频重试，仅保持低频心跳，等待 Windows 网卡事件（重新插拔网线、重新连接 WiFi）再次无延迟激活。

---

## 4. 技术栈选型与 Cargo 依赖红线 (Dependencies Rules)

为保持极致小巧和无运行时开销，依赖必须精挑细选：
- **HTTP / 网络**：优先使用轻量同步库 `ureq`（内置 rustls，无异步运行时巨大开销，常驻内存 < 2MB）或 `reqwest`（无 default-features, 启用 blocking 或精简 tokio）。
- **托盘图标与窗口**：`tray-icon` + `winit` / 原生轻量 Win32 消息循环（通过 `windows-sys` 或 `winapi`），杜绝任何完整 GUI 框架。
- **配置序列化**：`serde` + `toml`。
- **Windows API**：`windows-sys`（仅引入所需 API 模块，如 Win32_System_Registry, Win32_NetworkManagement_IpHelper）。

### `Cargo.toml` 极致优化配置范例
```toml
[profile.release]
opt-level = "z"     # 针对二进制体积进行极致优化
lto = true          # 启用全局链接时优化
codegen-units = 1   # 最大化内联与剪裁
panic = "abort"     # 禁用栈展开以精简体积
strip = true        # 剥离所有调试符号
```

---

## 5. Agent 编码与审查自检清单 (Verification Checklist)

当 Agent 编写或修改代码时，必须完成以下核验：
- [ ] **无 panic 隐患**：全局搜索未发现未校验的 `.unwrap()` 或 `.expect()`。
- [ ] **路径安全**：所有文件读写均已通过 `get_app_dir()` 锚定绝对路径。
- [ ] **0 延迟感知**：网络适配器变更时能在 100ms 内触发重测，而非死等下一个轮询周期。
- [ ] **重定向隔离**：HTTP 探测请求已确认关闭跟随重定向（`redirect: false`）。
- [ ] **开机自启隔离**：注册表项写入的是带双引号的完整绝对路径 `\"C:\\path\\to\\NetTrigger.exe\"`。
- [ ] **静态资源嵌入**：托盘的默认图标可通过 `include_bytes!` 静态嵌入二进制，即使外部缺少 `.ico` 也能安全自愈运行。
- [ ] **Git 身份合规**：所有提交必须以 `Sacilave <sacilave@gmail.com>` 签署，严禁关联旧仓库。
- [ ] **版本发布与文档全量同步**：每次发布新版本（Release）时，必须同步更新以下全链路要素：
  1. `Cargo.toml` 中的 `version = "x.y.z"`；
  2. `installer/NetTrigger.iss` 中的默认版本号；
  3. `README.md` 与 `README_EN.md` 中的 Release 徽标及下载表格直链（确保用户点击直链下载到当前最新版）；
  4. 产物命名统一采用规范格式（`NetTrigger-vX.Y.Z-...`），严禁上传未带版本号的重复副本避免用户混淆。
