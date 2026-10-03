# NetTrigger 系统整体架构与泛用性扩展设计

> 本文档深入剖析 NetTrigger 的核心软件架构模式、分层解耦模型以及泛用性扩展蓝图，确立其作为**通用网络事件驱动自动化插件引擎**的技术基石。

---

## 1. 软件架构哲学：从“单一校园网脚本”到“通用网络事件引擎”

传统的掉线重连工具通常将“探测、解析、重连”紧紧耦合为一个专有过程。而 NetTrigger 采用现代系统级架构模式，将系统抽象为三元组：

$$\text{System} = \langle \text{State Engine}, \text{Trigger Pipeline}, \text{Action Matrix} \rangle$$

```mermaid
graph TD
    subgraph Sources [事件输入源 (Event Sources)]
        S1[Windows IP Helper 事件<br/>NotifyIpInterfaceChange]
        S2[低频心跳定时器<br/>Waitable Timer]
        S3[用户主动操作<br/>托盘菜单点击]
    end

    subgraph Core [NetTrigger 状态中枢 (Core Engine)]
        FSM[核心状态机 (FSM)]
        Probe[权威 204 探针<br/>(Captive Portal Probe)]
        Decider[状态判定与退避器<br/>(State Decider)]
    end

    subgraph Actions [泛用性动作矩阵 (Action Matrix)]
        A1[HTTP 静默登录认证<br/>(Silent Auth Client)]
        A2[系统默认浏览器唤起<br/>(Browser Launcher)]
        A3[系统命令 / 脚本执行<br/>(Command Runner)]
        A4[Windows 系统通知推送<br/>(Toast / Balloon Notification)]
        A5[系统 DNS 缓存刷新<br/>(Flush DNS Resolver)]
    end

    Sources -->|事件信号| FSM
    FSM <-->|执行裁决| Probe
    FSM -->|状态跃迁| Decider
    Decider -->|分派动作| Actions
```

---

## 2. 核心分层解耦体系 (Layered Architecture)

### 2.1 状态层 (State Layer)
- **职责**：维护单一可信状态源（Single Source of Truth），实现严格的有限状态机流转。
- **状态定义**：
  - `Online`：互联网完全连通（通过 204 端点验证）。
  - `CaptivePortal`：物理网络畅通，但处于网关拦截/需要认证状态。
  - `Authenticating`：正在分派动作执行认证。
  - `Disconnected`：无物理网卡连接或网线断开。
  - `BackoffCooldown`：认证连续失败，进入防风暴挂起状态。
- **不可变性与线程安全**：状态机采用消息通道（Channel）驱动，消除多线程状态竞争。

### 2.2 探针层 (Probe Layer)
- **职责**：纯粹执行网络连通性裁决，不产生任何副作用。
- **关键设计**：
  - 严格禁用跟随 HTTP 30x 重定向；
  - 默认采用 RFC 推荐的标准 204 No Content 端点（极低开销，无需解析 Body）；
  - 支持多端点融合仲裁（Primary 204 + Fallback 微软 NCSI），防止单个探测服务器单点故障。

### 2.3 动作执行矩阵 (Action Layer)
- **职责**：动作执行器实现为可插拔的 `Action` 特征（Trait）。
- **通用动作类型**：
  1. `HttpAction`：针对大多数 Web Portal 认证系统（包含表单提交、JSON 提交、Header 注入与动态宏替换）。
  2. `BrowserAction`：调用 Win32 原生 `ShellExecuteW` 打开默认浏览器。
  3. `CommandAction`：执行本地外部命令或脚本（如 `ipconfig /renew`、自定义 powershell 脚本）。
  4. `NotificationAction`：发送系统级轻量气泡通知。

---

## 3. 泛用性与扩展能力 (Extensibility)

### 3.1 泛用性场景支持
NetTrigger 不仅适用于学校园区网，其泛用性架构天然支持：
1. **企业办公网络 Captive Portal 认证**：公司访客 WiFi 或内网接入超时自动续约。
2. **酒店/机场/高铁 WiFi 拦截自动唤醒**：连接后即刻捕获 302 页面，自动完成提示或重连。
3. **家庭/实验室路由掉线守护**：当软路由或宽带掉线时，自动执行本地脚本重拨号（PPPoE）或重启网络接口。

### 3.2 动态宏与上下文感知系统
在动作执行时，系统支持内置的动态上下文注入：
- `{ip}`：自动提取当前连通网卡的物理局域网 IP（如 `192.168.1.100` 或 `10.x.x.x`）。
- `{mac}`：自动格式化当前网卡 MAC 地址。
- `{time_ms}` / `{time_s}`：生成当前高精度时间戳（许多现代 Portal 认证必须防重放参数）。
- `{username}` / `{password}`：配置文件凭据安全解构。

---

## 4. 安全治理与信息防泄露规范 (Security & Privacy Governance)

1. **零硬编码承诺**：
   - 源代码中杜绝硬编码任何特定机构、学校、私人服务器的域名、IP、账号或特定系统标识。
   - 所有测试与文档中的示例地址均采用 RFC 2606 规定的保留域名：`example.com` / `example.edu` 与保留网段 `192.0.2.0/24`。
2. **密码存储与安全**：
   - 配置文件存储在用户本地 App 目录，权限受 Windows ACL 保护。
   - 支持从 Windows 凭据管理器（Credential Manager / DPAPI）解密凭据的扩展接口，保障高级安全需求。
3. **干净卸载与零残留**：
   - 关闭软件时自动清理系统托盘图标缓存。
   - 关闭开机自启时自动从注册表彻底移除键值，不留任何死链或系统垃圾。
