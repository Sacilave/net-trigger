# NetTrigger ⚡

**English** | [简体中文](README.md)

**A native, ultra-lightweight (RAM < 2MB), event-driven network status keeper and captive portal auto-reconnect engine designed for Windows.**

[![Release](https://img.shields.io/badge/Release-v1.2.0-blue.svg)](https://github.com/Sacilave/net-trigger/releases)
[![License](https://img.shields.io/badge/License-CC%20BY--NC%204.0-red.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011-blue.svg)]()
[![Memory](https://img.shields.io/badge/RAM-%3C%201.9%20MB-brightgreen.svg)]()
[![CPU](https://img.shields.io/badge/CPU-0.00%25%20(Idle)-brightgreen.svg)]()
[![Binary Size](https://img.shields.io/badge/Size-1.88%20MB%20(Single%20Exe)-success.svg)]()

> **NetTrigger** replaces traditional brute-force 1–2 second polling scripts and disruptive pop-up windows. By subscribing directly to Windows native IP Helper kernel notifications, NetTrigger passively wakes up the exact millisecond a network interface change or disconnection occurs. It authenticates silently in the background, **keeping online gaming, video conferences, and full-screen entertainment uninterrupted, zero-lag, and 100% focused**.

---

## 📌 Table of Contents

- [🚀 Quick Start (Download & Usage)](#-quick-start)
  - [Step 1: Download Program](#step-1-download-program)
  - [Step 2: Configure Authentication](#step-2-configure-authentication)
- [🏫 Gateway Support & Verification Status](#-gateway-support--verification-status)
- [💡 Performance & Resource Comparison](#-performance--resource-comparison)
- [⚡ Key Architecture & Features](#-key-architecture--features)
- [🎮 Operating Profile Presets](#-operating-profile-presets)
- [🖥️ System Tray Indicators & Menu](#️-system-tray-indicators--menu)
- [🌐 Multi-Language Support (i18n)](#-multi-language-support-i18n)
- [🛠️ Building from Source](#️-building-from-source)
- [📄 License & Non-Commercial Notice](#-license--non-commercial-notice)

---

## 🚀 Quick Start

### Step 1: Download Program

Choose the edition that best suits your workflow (**if unsure, choose the 1st one: Setup Installer**):

| Distribution | Target File (Which to download) | Direct Download Link | Best For | Features |
| :--- | :--- | :--- | :--- | :--- |
| **Setup Installer (Recommended ⭐)** | **`NetTrigger-Setup.exe`** | [**Direct Download**](https://github.com/Sacilave/net-trigger/releases/latest/download/NetTrigger-Setup.exe) | Most students & everyday users | Standard setup wizard, no admin UAC prompt needed. **Automatically creates Start Menu & Desktop shortcuts**; supports "Pin to taskbar". |
| **Portable Standalone Binary** | **`NetTrigger.exe`** | [**Direct Download**](https://github.com/Sacilave/net-trigger/releases/latest/download/NetTrigger.exe) | USB flash drive & portable usage | **Single file only ~1.88MB**, zero installation needed, extract and run anywhere without registry pollution. |
| **Full Portable Zip Package** | **`NetTrigger-windows-x64-portable.zip`** | [**Direct Download**](https://github.com/Sacilave/net-trigger/releases/latest/download/NetTrigger-windows-x64-portable.zip) | Users wanting binary + config template + docs | Contains `NetTrigger.exe`, `config.example.toml` template, and documentation bundled together. |

> 📌 **Quick Tips for Beginners**:
> 1. If browsing GitHub Releases and unsure which file to pick, look for the file ending in **`.exe`** (choose **`NetTrigger-Setup.exe`**);
> 2. For all releases and detailed changelogs, visit [GitHub Releases](https://github.com/Sacilave/net-trigger/releases).
> 3. 💡 **Pure & Lightweight Guarantee**: NetTrigger uses only ~1.8MB of RAM, 0.00% idle CPU, and operates cleanly with standard Windows user privileges (no administrator rights needed).

### Step 2: Configure Authentication (Choose One of Two Modes)

Supports two quick scenarios: **Campus Network** and **Public Network**.

#### Mode A: Browser Auto-Login / Remember Password (Default ⭐)
If your campus, hotel, or dorm network supports remembering passwords in the browser:
1. **Left-click** or **right-click** the tray icon ➔ Click **【Settings】**;
2. Enter your captive portal login URL (or leave empty to let NetTrigger auto-detect upon disconnection);
3. Click **【Save Settings】**.

#### Mode B: Silent Background HTTP Authentication (Gaming / Zero-Distraction)
If you want 100% foreground focus retention without any browser windows popping up:
1. Open **【Settings】** ➔ Switch to the **【Silent Auto-Login (Recommended · No Popup)】** tab;
2. Press `F12` on your browser's login page, copy the login request as cURL, and paste it into the cURL auto-importer;
3. Click **【Test Connection】** to verify gateway responses ➔ Click **【Save Settings】**.

---

## 🏫 Gateway Support & Verification Status

| Gateway / Architecture | Support Level | Real-World Test Status | Recommended Usage |
| :--- | :--- | :--- | :--- |
| **Ruijie (RG-SAM+ / eportal)** | **Locally Verified & Guaranteed** | **Fully tested and verified in physical campus network** (covers endpoint login, dynamic IP macro substitution, success.jsp sanitization, etc.) | Click "Ruijie Auto-Fill" preset button in Settings and enter password |
| **Srun (深澜软件)** | **Standard Protocol Support** | Not verified on physical hardware; 100% compatibility cannot be guaranteed | Press F12, Copy as cURL, and auto-import |
| **Dr.COM (城市热点)** | **Standard Protocol Support** | Not verified on physical hardware; 100% compatibility cannot be guaranteed | Press F12, Copy as cURL, and auto-import |
| **Huawei / Public Wi-Fi / Others** | **Standard Protocol Support** | Not verified on physical hardware; 100% compatibility cannot be guaranteed | Standard POST/GET supported; use provided AI prompt if encountering proprietary tokens |

---

## 💡 Performance & Resource Comparison

| Metric | Traditional Scripts / Python / .NET | Electron / Web-Wrapper Apps | **NetTrigger** |
| :--- | :--- | :--- | :--- |
| **Working Set Memory** | 35 MB ~ 60 MB | 80 MB ~ 150 MB+ | **1.5 MB ~ 1.9 MB (Ultra-Lightweight)** |
| **CPU Usage (Idle)** | 0.5% ~ 3.0% (Tight polling loops) | 1.0% ~ 5.0% | **Strictly 0.00% (Kernel event suspended)** |
| **Disconnect Perception**| 1.0s ~ 2.0s (Fixed polling interval)| 2.0s ~ 5.0s | **< 5ms (Windows IP Helper wake-up)** |
| **Focus Disruption** | Pops up full-screen browser window | Window or notification steals focus | **100% silent background request, zero stealing** |
| **False-Online Detection**| ❌ Often mistakes HTTP 302 for 200 OK | Relies on DOM loading | **Authoritative 204 endpoint + FollowRedirects=false** |
| **Binary Footprint** | Requires Python / .NET runtimes | 60 MB ~ 120 MB | **~1.88 MB (Single-file native binary)** |

---

## ⚡ Key Architecture & Features

```mermaid
flowchart LR
    A[Windows IP Helper Event] -->|< 5ms Passive Wakeup| B[50ms Link Debounce]
    B --> C[Authoritative 204 Probe]
    C -->|Gateway Interception Detected| D[Silent HTTP / Browser Reconnect]
    D -->|Quick Verification| E[🟢 Online Restored (< 95ms total)]
    style E fill:#22c55e,stroke:#15803d,color:#ffffff
    style D fill:#0284c7,stroke:#0369a1,color:#ffffff
```

1. **Sub-millisecond Event Wakeup & 50ms Golden Debounce**: Hooks into Windows native IP Helper (`NotifyAddrChange`). Reacts instantaneously (<5ms) when plugging in Ethernet or switching Wi-Fi. The 50ms link stabilization window allows local DHCP and routing to settle before firing requests, eliminating request storms.
2. **Zero-Focus Stealing Guarantee**: Background network requests execute quietly without spawning browser processes, ensuring competitive games and remote meetings never lose mouse focus or drop frames.
3. **On-Demand Ephemeral Web Control Center**: No built-in browser runtimes. Clicking 【⚙️ Settings】 temporarily launches a local lightweight socket server that opens in your default browser. Closing the tab or leaving it idle for 5 minutes cleanly shuts down the listener, dropping memory right back to ~1.8MB.
4. **Industrial-Grade Crash Resilience**: Zero unchecked `.unwrap()` or `.expect()` calls in runtime flows. Single-instance global mutex prevents accidental duplicates, and startup registry writes are safely sandboxed in `HKCU`.

---

## 🎮 Operating Profile Presets

Three optimized profiles are available in Settings with a single click:

| Profile | Debounce Window | Heartbeat Interval | Ideal Use Case |
| :--- | :--- | :--- | :--- |
| **`gaming` (Gaming Instant)** | **50ms (Instant response)** | Every 15s | Online competitive gaming, Discord/Zoom calls. Zero delay, strict silent mode. |
| **`balanced` (Balanced Recommended)**| **150ms** | Every 45s | Daily dorm, office, and home usage. Balances reconnect speed with AP load. |
| **`power_save` (Battery Saver)**| **500ms** | Every 90s | Library, coffee shops, and mobile laptops. Minimizes radio wakeups for extended battery life. |

---

## 🖥️ System Tray Indicators & Menu

### Color Ring Indicator (Dynamically generated in memory, zero .ico dependency)
- 🟢 **Emerald Green**: `Connected` (Hover tooltip shows round-trip latency, e.g., `Latency: 12ms`)
- 🟡 **Vibrant Amber**: `Connecting...` or `Captive Portal detected, connecting...`
- 🔴 **Warning Red**: `Disconnected (No Wi-Fi/Ethernet)` or `Retry in cooldown...`
- 🔵 **Sky Blue**: `Starting up...`

### System Tray Shortcuts
- **Left-Click / Double-Click**: Instantly opens the **【Settings】** control center;
- **Right-Click**: Opens a clean context menu:
  ```
  +------------------------------------+
  | Reconnect Now                      |  <-- Immediately triggers re-probe & reconnect
  | Open Login Portal                  |  <-- Quick browser login portal channel
  |------------------------------------|
  | Settings                           |  <-- Opens Web control center (also on left-click)
  | Language / 语言          ▶         |  <-- In-place language switch (Auto / English / 简体中文)
  |------------------------------------|
  | [√] Start on Boot                  |  <-- Toggle startup registry (no admin required)
  | [√] Silent Mode (Do Not Disturb)   |  <-- Suppress toast notifications on reconnect
  |------------------------------------|
  | Quit                               |
  +------------------------------------+
  ```

---

## 🌐 Multi-Language Support (i18n)

NetTrigger features full zero-overhead internationalization:
- **Instant In-Place Tray Submenu**: Right-click the tray icon and hover over `🌐 Language / 语言` to switch on the fly between `Auto (自动)`, `English`, and `简体中文`. It re-renders menus and tooltips in real time and automatically persists your selection to `config.toml` without launching a browser.
- **Automatic System Language Detection**: On startup, NetTrigger reads `GetUserDefaultUILanguage()`. Chinese systems automatically load Chinese tray labels, tooltips, and configuration templates; all other systems default to English.
- **Language Switcher in Settings**: You can also toggle language in the Web Settings header.
- **Region-Aware Config Generation**: On first run on English systems, `config.toml` is generated with full English documentation and global Anycast probe endpoints (`connectivitycheck.gstatic.com`).

---

## 🛠️ Building from Source

Requirements: Standard Rust toolchain (Edition 2021).

```powershell
# 1. Clone repository
git clone https://github.com/Sacilave/net-trigger.git
cd net-trigger

# 2. Run the automated test suite (36 unit and mock tests)
cargo test

# 3. Compile optimized release binary (with LTO, Strip, and size-optimization)
cargo build --release

# The compiled single-file binary will be located at target/release/NetTrigger.exe (~1.88 MB)
```

---

## 📄 License & Non-Commercial Notice

This project is licensed under the **[Creative Commons Attribution-NonCommercial 4.0 International (CC BY-NC 4.0)](./LICENSE)** License.

### ⚠️ Strict Non-Commercial Terms:
1. **Commercial Use Prohibited**: Individuals, institutions, or enterprises may NOT use this software, source code, or derivatives for **commercial sales, paid bundling, paid distribution, or for-profit services**;
2. **Non-Commercial Self-Use Allowed**: Students, researchers, and developers are welcome to freely use, modify, and share this project for educational, personal, and non-commercial purposes;
3. **Attribution**: Redistributions must retain author attribution (`Sacilave <sacilave@gmail.com>`) and this non-commercial license.

Copyright © 2026 Sacilave (<sacilave@gmail.com>). All rights reserved.
