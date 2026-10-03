# NetTrigger ⚡

**原生极轻量 · 毫秒级 0 延迟 · 电竞级静默防夺焦 · Captive Portal 网络事件自动化守护引擎**

[![Language](https://img.shields.io/badge/Language-Rust%202021-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011-blue.svg)]()
[![Memory](https://img.shields.io/badge/RAM-%3C%201.9%20MB-brightgreen.svg)]()
[![CPU](https://img.shields.io/badge/CPU-0.00%25%20(Idle)-brightgreen.svg)]()
[![Binary Size](https://img.shields.io/badge/Size-1.83%20MB%20(Single%20Exe)-success.svg)]()
[![License](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)]()

> **NetTrigger** 是一款专为 Windows 平台打造的原生单文件绿色守护引擎。告别传统脚本 1~2 秒高频死循环轮询与粗暴弹窗切屏，NetTrigger 接入 Windows 原生网络内核通知，在掉线或网络变更的瞬间（毫秒级）被动唤醒并后台静默完成认证，**让打游戏、全屏观影与远程会议全程流畅、零弹窗、零掉帧、零感不断流**。

---

## 💡 为什么选择 NetTrigger？

| 核心指标 | 传统简易脚本 / Python / .NET | Electron / 网页容器工具 | **NetTrigger (本引擎)** |
| :--- | :--- | :--- | :--- |
| **常驻物理内存** | 35 MB ~ 60 MB | 80 MB ~ 150 MB+ | **1.5 MB ~ 1.9 MB (极致轻盈)** |
| **CPU 占用 (平衡态)**| 0.5% ~ 3.0% (高频死循环轮询) | 1.0% ~ 5.0% | **严格 0.00% (Windows 内核被动挂起)** |
| **掉线感知延迟** | 1.0 秒 ~ 2.0 秒 (固定轮询间隔)| 2.0 秒 ~ 5.0 秒 | **< 5 毫秒 (内核级异步通知)** |
| **重连打扰体验** | 粗暴弹出系统浏览器全屏切屏 | 弹出窗口或通知气泡 | **纯后台静默报文，100% 保持游戏焦点** |
| **防假在线识别** | ❌ 易被 302 劫持误判为 200 正常 | 依赖网页加载 | **权威 204 端点 + 显式禁用重定向判定** |
| **单文件绿色体积** | 需安装 Python / .NET 运行时 | 60 MB ~ 120 MB | **仅 1.83 MB (单文件免安装，解压即用)** |

---

## ⚡ 核心产品特性

```mermaid
flowchart LR
    A[Windows IP Helper 内核通知] -->|< 5ms 被动唤醒| B[50ms 黄金链路防抖]
    B --> C[权威 204 探针检测]
    C -->|发现 302 拦截| D[后台纯静默 HTTP POST 认证]
    D -->|二次快速验证| E[🟢 瞬间恢复连通 (总耗时 < 95ms)]
    style E fill:#22c55e,stroke:#15803d,color:#ffffff
    style D fill:#0284c7,stroke:#0369a1,color:#ffffff
```

### 1. 电竞级毫秒响应 · 50ms 黄金防抖
- **内核驱动，拒绝盲目轮询**：挂钩 Windows IP Helper API（`NotifyAddrChange`），线程利用系统内核事件句柄无损等待。网卡没有插拔、IP 没有变更时，**CPU 开销绝对为 0.00%**。
- **瞬时一发必中**：从网卡感知断网到完成认证耗时 **< 95ms**，低于大部分游戏的 TCP 重传超时阈值，游戏内仅体现为极微小的 Ping 抖动，不退房间、不掉排位。
- **50ms 链路稳定防抖（Debounce）**：物理网卡建立连接后智能微等待，确保 DHCP 分配与本地网关路由完全就绪，杜绝无效发包。

### 2. 绝对静默与防夺焦保护 (Zero-Focus Stealing)
- **纯后台报文注入**：所有认证动作由轻量 HTTP 管道直接与网关通信，桌面前台全屏游戏/专业生产力软件**绝不失去鼠标焦点、绝不最小化、绝不切屏**。
- **气泡与弹窗硬屏蔽**：在静默模式与电竞模式下，即使网络重新连接成功，也绝不弹出系统通知气泡打扰用户。

### 3. 按需瞬态微型 Web 控制中心 (On-Demand Ephemeral Local Web)
- **平时 0 开销**：不内置 Chromium / WebView2，平时不监听端口、不占多余内存。
- **点击即唤起**：从托盘右键点击【⚙️ 可视化配置中心】时，通过本地临时回环端口拉起默认浏览器，渲染静态嵌入的现代化 Fluent / Tailwind 风格控制面板。
- **🪄 F12 cURL 一键智能导入**：在浏览器登录页面按 F12 复制 cURL 命令，粘贴即可**自动解析出 URL、参数、账号密码并填充表单**，小白 10 秒搞定！
- **动态宏点选与一键测试**：界面自带 `{ip}`、`{mac}`、`{time}` 点击插入，支持免断网一键测试当前配置有效性。配置完成或 5 分钟空闲后微型服务自动退出，**常驻内存秒回 1.5MB**。

### 4. 工业级鲁棒性与免提权自启
- **Panic-Free 防崩保证**：全业务逻辑 0 未受保护的 `unwrap()` / `expect()`，任何 DNS 超时、网关掉线均由状态机安全消化。
- **单实例互斥锁**：系统级 `Global\NetTrigger_SingleInstance_Mutex`，防止多开。
- **免 UAC 提权自启动**：纯净读写当前用户注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，开机启动路径带绝对路径防空格转义，彻底免疫工作目录偏移。

---

## 🚀 30 秒快速上手

### 步骤 1：下载并运行
1. 下载最新的单文件可执行程序 [`NetTrigger.exe`](./target/release/NetTrigger.exe)；
2. 双击打开，程序会自动在同级目录生成带有详尽中文注释的标准 `config.toml`，并静默常驻系统托盘。

### 步骤 2：配置认证信息 (两种方式任选)

#### 方式 A：现代化可视化网页配置 (小白首选，推荐 ⭐)
1. 鼠标右键桌面右下角 NetTrigger 托盘图标 ➔ 点击 **【⚙️ 可视化配置中心 (Web)】**；
2. 在自动打开的浏览器控制中心中，使用 **F12 cURL 一键导入**，或手动填入账号密码；
3. 点击 **【🧪 一键诊断与联通性测试】** 确认网关返回成功 ➔ 点击 **【💾 保存并立即生效】** 即可！

#### 方式 B：记事本直接编辑 (极客首选)
右键托盘图标 ➔ 点击 **【📝 直接编辑 config.toml】**，填入你的认证接口与账号密码：

```toml
[general]
# 运行档位: "gaming" (电竞极速/50ms防抖/15s心跳) | "balanced" (平衡/150ms防抖/45s心跳)
profile = "gaming"
silent_mode = true

[auth]
mode = "http"
portal_url = "http://portal.example.edu/"

[auth.http]
method = "POST"
action_url = "http://portal.example.edu/api/login"

[auth.http.params]
username = "your_student_id"
password = "your_password"
# 支持动态宏注入: {ip} 本机内网IP / {mac} 网卡物理地址 / {time} 毫秒时间戳
wlanuserip = "{ip}"
```

---

## 🎮 运行档位预设 (Profile Presets)

NetTrigger 拒绝过度设计，内置 3 种开箱即用的精密平衡档位，无需纠结复杂底层参数：

| 档位名称 | 物理链路防抖 | 心跳保活周期 | 适用场景特点 |
| :--- | :--- | :--- | :--- |
| **`gaming` (电竞极速)** | **50ms (瞬时响应)** | 15 秒一次 | 打联机游戏、远程会议必备。一秒不耽误，强力静默绝不切屏。 |
| **`balanced` (平衡推荐)**| **150ms** | 45 秒一次 | 日常宿舍、工位办公推荐，平衡网络连通与无线 AP 负载。 |
| **`power_save` (省电办公)**| **500ms** | 90 秒一次 | 图书馆便携使用，最大化延长笔记本电池续航。 |

---

## 🖥️ 系统托盘视觉与交互说明

系统托盘图标采用**纯代码在内存中动态渲染的矢量发光图标**，不依赖外部 `.ico` 文件，永不丢失：

- 🟢 **翠绿色发光**：`网络畅通`（鼠标悬停 Tooltip 即时显示当前往返延迟，如 `延迟: 14ms`）
- 🟡 **金黄色发光**：`正在自动认证...`（检测到网络被网关拦截，正在后台毫秒级重连）
- 🔴 **警示红点**：`网络已断开` 或 `认证多次失败退避等待`
- 🔵 **天空蓝**：`系统启动初始化中`

### 右键托盘上下文菜单
```
+------------------------------------+
| ⚡ 立即检测并重新认证               |  <-- 手动立即强制重测并重连
| 🧪 一键测试当前认证配置             |  <-- 免断网验证账号密码接口是否有效
| 🌐 打开网络认证网页 (Browser)       |  <-- 浏览器快速登录通道
|------------------------------------|
| ⚙️ 可视化配置中心 (Web)            |  <-- 打开现代 Fluent 控制台 (带 cURL 导入)
| 📝 直接编辑 config.toml             |  <-- 记事本极客直编
|------------------------------------|
| [√] 开机自启动                     |  <-- 点击切换当前用户注册表自启
| [√] 静默模式 (游戏免打扰)          |  <-- 切换是否屏蔽所有桌面气泡通知
|------------------------------------|
| 🚪 退出 NetTrigger                 |
+------------------------------------+
```

---

## 🏗️ 核心架构与模块边界

```
NetTrigger/
├── Cargo.toml                 # 极限 LTO + Strip + opt-level="z" 编译优化
├── AGENTS.md                  # 架构治理守则与编码红线
├── src/
│   ├── main.rs                # 命名互斥锁、Win32 消息泵、顶层 FSM 事件循环
│   ├── config.rs              # 配置解析、自愈模板、电竞预设、宏参数注入
│   ├── state.rs               # 核心有限状态机 (FSM) 与指数退避调度器
│   ├── probe.rs               # RFC 204 权威探针 (禁用重定向，微软 NCSI 兜底)
│   ├── watcher/
│   │   ├── mod.rs
│   │   └── win_network.rs     # Windows IP Helper 原生事件监听与 50ms 黄金防抖
│   ├── auth/
│   │   ├── mod.rs             # 认证执行调度器
│   │   ├── http_client.rs     # 后台静默 HTTP POST/GET 与表单/JSON编码
│   │   └── browser.rs         # 浏览器唤起 (受电竞静默模式严格门控保护)
│   ├── tray/
│   │   ├── mod.rs             # 托盘管理、内存 RGBA 图标生成、上下文菜单
│   │   └── autostart.rs       # HKCU\Run 注册表开机自启安全管理
│   ├── web_config/
│   │   ├── mod.rs             # 瞬态微型原生 HTTP 服务 (TcpListener)
│   │   └── template.html      # 嵌入式响应式 Fluent 控制中心 UI (含 cURL 解析)
│   └── utils/
│       ├── fs.rs              # 绝对路径锚定 (get_app_dir)
│       └── logger.rs          # 极简轻量滚动日志
└── docs/                      # 架构设计详档与技术白皮书
```

---

## 🛠️ 本地编译构建

本项目基于标准 Rust 工具链（Edition 2021）。

```powershell
# 1. 克隆代码仓库
git clone https://github.com/Sacilave/NetTrigger.git
cd NetTrigger

# 2. 运行完整单元测试套件 (包含所有 mock 与边界测试)
cargo test

# 3. 编译发布版 (开启全部体积裁剪与 LTO 优化)
cargo build --release

# 产物即位于 target/release/NetTrigger.exe (体积仅 ~1.83MB)
```

---

## 📖 设计规范详档索引

- 治理守则与红线：[`AGENTS.md`](./AGENTS.md)
- 完整需求与架构规格：[`docs/REQUIREMENTS_AND_ARCHITECTURE.md`](./docs/REQUIREMENTS_AND_ARCHITECTURE.md)
- 性能工程与零延迟机制：[`docs/PERFORMANCE_AND_ZERO_LATENCY.md`](./docs/PERFORMANCE_AND_ZERO_LATENCY.md)
- 人性化交互与易用度设计：[`docs/UX_AND_USABILITY_DESIGN.md`](./docs/UX_AND_USABILITY_DESIGN.md)
- 系统架构与泛用性扩展：[`docs/SYSTEM_ARCHITECTURE_AND_EXTENSIBILITY.md`](./docs/SYSTEM_ARCHITECTURE_AND_EXTENSIBILITY.md)

---

## 📄 开源许可证

本项目基于 [MIT](./LICENSE-MIT) 或 [Apache-2.0](./LICENSE-APACHE) 双重许可证开源。  
Copyright © 2026 Sacilave (<sacilave@gmail.com>).
