# NetTrigger (网络事件自动化触发与认证守护引擎)

[![Language](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2B-blue.svg)]()
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green.svg)]()

> **NetTrigger** 是一款专为 Windows 平台打造的**极轻量级（常驻物理内存 < 2.5MB、平衡态 CPU 严格 0.00%）、原生零依赖（单文件绿色 exe）、基于事件驱动（毫秒级 0 延迟响应）的网络状态自动化触发与 Captive Portal 认证守护引擎**。

---

## 📖 核心设计文档索引 (Documentation)

本项目已完成完整的需求工程、性能架构及人机交互规范设计，所有开发工作必须严格遵守以下文档体系：

| 文档名称 | 路径 | 核心概述 |
| :--- | :--- | :--- |
| **Agent 架构治理准则** | [`AGENTS.md`](./AGENTS.md) | AI 编程助手与开发者协同规范、架构边界、性能红线与编码自检清单 |
| **需求定义与架构规格** | [`docs/REQUIREMENTS_AND_ARCHITECTURE.md`](./docs/REQUIREMENTS_AND_ARCHITECTURE.md) | 总体需求规格、旧系统缺陷分析、FSM 状态机与配置规范 |
| **性能工程与零延迟机制** | [`docs/PERFORMANCE_AND_ZERO_LATENCY.md`](./docs/PERFORMANCE_AND_ZERO_LATENCY.md) | Windows IP Helper 原生事件监听、CPU 0% 休眠、内存 < 2.5MB 控制体系 |
| **人性化交互与易用度设计** | [`docs/UX_AND_USABILITY_DESIGN.md`](./docs/UX_AND_USABILITY_DESIGN.md) | 托盘四态指示、右键菜单、一键诊断测试、免打扰与配置模板自愈 |
| **系统架构与泛用性扩展** | [`docs/SYSTEM_ARCHITECTURE_AND_EXTENSIBILITY.md`](./docs/SYSTEM_ARCHITECTURE_AND_EXTENSIBILITY.md) | 解耦架构模式、泛用性场景、多级探针融合与信息脱敏规范 |

---

## ⚡ 核心特性一览

1. **毫秒级事件驱动（近乎 0 延迟）**：
   - 挂钩 Windows 底层 IP Helper API（`NotifyIpInterfaceChange`），网卡连接或 IP 变更瞬间毫秒级被动唤醒，杜绝 1~2 秒高频死循环轮询。
2. **极致低占用（打游戏完全无感）**：
   - **常驻物理内存**：稳定控制在 **1.5MB ~ 2.5MB** 之间。
   - **CPU 占用**：平衡态完全利用内核事件对象挂起，严格保持 **0.00%**。
   - **单文件免安装**：静态编译剥离调试符号后体积 **< 2.0MB**，图标及核心资源全静态嵌入。
3. **权威探针防劫持误判**：
   - 严格禁用跟随 HTTP 30x 重定向；结合国内高速 204 端点与微软 NCSI 端点，精准识别校园网/公共网 Captive Portal 拦截，彻底根除“假连通”现象。
4. **两级自动化认证链路**：
   - **Tier-1 静默认证（首选）**：后台自动发送 HTTP POST/GET 完成网络认证，打游戏、看视频完全不弹窗、不夺取焦点、不掉帧。
   - **Tier-2 浏览器兜底（Fallback）**：若未配置账号密码，自动通过默认浏览器唤起登录门户。
5. **小白友好与极致人性化**：
   - 双击即用：无配置时自动生成带详尽中文注释的 `config.toml`；
   - 托盘右键自带“一键测试当前配置”，无需等断网即可验证账号密码是否有效；
   - 一键开机自启动（安全写入当前用户注册表，无需管理员提权）。

---

## 🛠️ 当前工程状态与开发路线图

当前工程处于：**Phase 1（架构、性能与交互规范设计）已 100% 完美完成**。

### 下一阶段开发任务 (Phase 2):
1. **初始化 Cargo.toml**：引入 `ureq`、`windows-sys`、`tray-icon` 等轻量依赖，配置 release 体积裁剪。
2. **实现 `config.rs` 与 `utils/`**：绝对路径自愈解析、配置文件自动生成与模板。
3. **实现 `probe.rs`**：权威 204 探针与防重定向连通性判决器。
4. **实现 `watcher/`**：Windows 原生网络事件异步监听。
5. **实现 `state.rs` 与 `auth/`**：有限状态机流转与静默 HTTP 登录。
6. **实现 `tray/`**：Windows 托盘与右键菜单交互。

---

## 💡 新对话启动指引 (For New Chat Session)

在新对话中，你可以直接发送如下指令启动开发：

```markdown
请阅读 NetTrigger/ 目录下的 AGENTS.md 以及 docs/ 下的所有架构设计文档。
当前架构设计与规范已全面完成，请开始 Phase 2 工程代码落地：初始化 Cargo.toml 并实现核心配置与权威探针模块。
```
