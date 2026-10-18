# Win-Top

![Platform](https://img.shields.io/badge/platform-Windows%2010%2B%20x64-0078d4)
![Release](https://img.shields.io/github/v/release/nickshui/Win-Top)
![Tech](https://img.shields.io/badge/Tauri%20%2B%20Rust%20%2B%20Svelte-24292f)
![Native](https://img.shields.io/badge/no%20subprocess-PDH%20%2F%20ETW%20%2F%20WMI%20%2F%20WinAPI-16a34a)

**Win-Top 是一款 Windows 系统监控与资源管理工具。** 它把平时要开一堆工具才能看清的东西——进程与 GPU 占用、端口占用、磁盘空间、网络吞吐、开机启动项——收进一个桌面工作台，并提供 AI 辅助判断。

所有数据**直调 Windows 原生接口**（PDH 性能计数器 / ETW / WMI / WinAPI），**不派生 `netstat`、`tasklist`、`powershell` 等任何子进程**；默认普通权限运行即可用，只有 ETW 实时事件与磁盘温度需要管理员。

> 适用场景：任务管理器看不够细的时候（**每进程 GPU 占用与显存**、端口到底被谁占着、磁盘空间去哪了、开机为什么变慢）；也适合作为一台 Windows 机器的常驻健康看板。

![Win-Top 概览：CPU / 内存 / 磁盘 I/O / 网络 / GPU 一屏总览](docs/screenshots/01-overview.png)

---

## 功能一览

| 模块 | 能做什么 |
| --- | --- |
| **概览** | CPU / 内存 / 页面文件 / 磁盘 I/O / 网络吞吐一屏总览；**独立 GPU 卡片（按显卡分页：总览 + 每张卡一个 Tab）**；进程磁盘 I/O TOP 5；可交互历史趋势图 |
| **进程管理** | 全量进程列表 / 树形双视图；按名称或 PID 过滤；按 **CPU / 内存 / GPU / 显存** / 线程 / 磁盘读写排序；详情面板给出命令行、完整路径、句柄数、优先级、**GPU 逐卡分解**；行内结束进程或改优先级 |
| **实时事件** | 基于 ETW 的进程启动 / 退出实时流（需管理员），支持按进程名过滤 |
| **网络与端口** | 一键网络体检（本机 IP / 网关 / 公网 IP / 延迟 / DNS）；多路并发测速；连通性检测；**端口 → 进程 → PID** 映射与一键释放；基于 ETW 的每进程实时上下行速率 |
| **磁盘管理** | 分区容量与使用率；物理磁盘 SMART 健康与温度（温度需管理员）；基于 NTFS MFT 的整卷空间分析，百万级文件数秒完成，支持逐级下钻 |
| **优化加速** | 一键加速（垃圾清理 + 内存释放）；系统体检；8 类系统垃圾清理；开机启动项开关 |
| **系统工具** | 防火墙规则查看与开关；系统还原点；文件占用解锁；系统快照导出（JSON / CSV） |
| **AI 助手** | 接入任意 OpenAI 兼容接口（预置 8 家厂商）；每次提问自动附带真实系统快照，基于实测数据给建议，并优先引导使用软件内已有功能 |

> 全部 9 张截图见 [`docs/screenshots/`](docs/screenshots/)。

---

## GPU 监控（v0.1.6 新增）

Windows 的 GPU 性能计数器按 **(进程, 显卡, 引擎)** 暴露实例，Win-Top 直接读这套数据，因此能做到任务管理器之外的粒度：

![概览 GPU 卡片：总览 + 每张独立显卡一个 Tab](docs/screenshots/08-gpu.png)

- **每进程 GPU 占用**：取「最忙引擎」口径（与任务管理器一致），并可展开看该进程在各张卡上的分解；
- **每进程显存**：专用 / 共享分列，支持排序；
- **概览 GPU 卡片**：一张总览 + 每张独立显卡一个 Tab，含各卡占用、引擎分解、专用/共享显存、该卡自己的进程 TOP 8；
- **历史趋势**：GPU 曲线与 CPU / 内存 / 网络并列，时间窗 1 分 ~ 30 天。

单卡 Tab 给出该卡自己的占用、引擎分解、专用 / 共享显存，以及该卡的进程 TOP 8：

![单卡 Tab：引擎分解与逐卡进程排行](docs/screenshots/09-gpu-single.png)

「进程管理」中 GPU 与显存是独立的可排序列，详情面板给出该进程的逐卡分解：

![进程管理：GPU 与显存列](docs/screenshots/02-processes.png)

采集口径上有一处**必须说明**的地方：各进程的「专用显存」**不能相加**当作显卡占用。跨进程共享的画面显存会被同时记在每个持有它的进程名下——本机实测各进程相加 10.24 GiB，而显卡自身只报 5.02 GiB（`dwm.exe` 单个进程就报 5.72 GiB，比整卡还多）。显卡级占用一律读 `\GPU Adapter Memory(*)`，进程级才读 `\GPU Process Memory(*)`。

---

## 历史趋势持久化（v0.1.6 新增）

历史数据不再只活在内存里，会以定长追加的方式落到本地磁盘：

- **位置**：`%LOCALAPPDATA%\WinTop\history\`（便携版写 exe 同级的 `WinTop-data\history\`）；
- **格式**：自研定长记录（64 B/条，逐条 CRC32），按小时/天分段，**不引入 SQLite 等任何新依赖**；
- **写入**：每 15 秒批量落盘，正常使用对磁盘的压力约 0.03%/秒；
- **保留**：原始 1 秒 × 7 天 → 1 分钟聚合 × 90 天，**硬上限 200 MiB**，超出按最旧段淘汰；
- **降级**：目录不可写、磁盘空间不足、已有实例占用时自动退回纯内存模式，不影响其它功能。

---

## 技术栈与架构

**技术栈**：Tauri 1.x · Rust · Svelte 4 · Vite

```
┌──────────────────────────────┐
│          UI Frontend          │
│       Svelte 4 + Vite         │
└─────────────┬────────────────┘
              │ Tauri Bridge（事件推送 + 命令调用）
┌─────────────▼────────────────┐
│        Rust Core Service      │
│  系统采集 / 系统操作 / 权限控制 │
└─────────────┬────────────────┘
              │ Windows APIs
┌─────────────▼────────────────┐
│   PDH / ETW / WMI / WinAPI    │
└──────────────────────────────┘
```

**数据采集**——全部直调 Windows 原生接口，不派生 `netstat` / `powershell` 等子进程：

| 能力 | 实现 |
| --- | --- |
| CPU / 磁盘 I/O | PDH 性能计数器 |
| 内存 | `GlobalMemoryStatusEx` |
| 进程枚举 | `NtQuerySystemInformation` |
| 进程操作 | `OpenProcess` / `TerminateProcess` / `SetPriorityClass` |
| **每进程 GPU 占用 / 显存** | **PDH `GPU Engine` / `GPU Process Memory` / `GPU Adapter Memory`** |
| **显卡名称与容量** | **DXGI `EnumAdapters1` + `DXGI_ADAPTER_DESC1`** |
| 端口连接 | `GetExtendedTcpTable` / `GetExtendedUdpTable` |
| 进程事件 / 每进程流量 | ETW（Kernel-Process / Kernel-Network） |
| SMART / 磁盘温度 | WMI |
| 空间分析 | NTFS MFT 直读 |
| 历史持久化 | 自研定长追加文件（无第三方存储依赖） |

**采集模型**：后端常驻采集线程分层采样（指标 1s、进程 2s、GPU 2s、磁盘 15s、事件实时），通过事件推送到前端，避免高频轮询。

**权限模型**：默认普通权限运行，核心功能全部可用；ETW 实时事件、磁盘温度需要管理员权限，应用内提供「以管理员重启」一键提权；结束进程、清理磁盘、防火墙变更等风险操作均有确认与提示。

---

## 安装

从 [Releases](https://github.com/nickshui/Win-Top/releases) 下载：

| 文件 | 说明 |
| --- | --- |
| `Win-Top_x.y.z_portable_x64.exe` | 绿色便携版，免安装，下载后双击即用（推荐快速体验） |
| `Win-Top_x.y.z_x64-setup.exe` | NSIS 安装包，推荐 |
| `Win-Top_x.y.z_x64_en-US.msi` | MSI 安装包 |

**系统要求**：Windows 10 1809+ / Windows 11，x64；依赖 WebView2 运行时（Windows 11 自带；使用安装包时若缺失会自动引导安装）。

> - 便携版不含 WebView2 引导：极少数未预装 WebView2 的系统（多为精简版 Windows 10）需先安装 [WebView2 运行时](https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/)，再运行便携版。
> - 安装包与便携版均暂未做代码签名，SmartScreen 可能提示「未知发布者」，选择「仍要运行」即可。

## 本地构建与启动

依赖：Node.js 18+、Rust stable（MSVC 工具链）。

```bash
npm install
npm run tauri dev     # 开发调试（Vite 热更新 + Rust 后端）
npm run tauri build   # 构建安装包，产物在 src-tauri/target/release/bundle/
```

发版流程见 [`docs/release-process.md`](docs/release-process.md)。

---

## 待做任务

- [ ] 命令与脚本工具箱：常用命令（ipconfig、sfc、dism 等）参数可视化、模板化、执行日志留存
- [ ] Ctrl+K 命令面板（全局搜索进程 / 端口 / 功能直达）
- [ ] 概览仪表盘增强（模块聚合 Bento 布局）
- [ ] 异常检测与自动化任务（高占用告警、定时清理）
- [ ] 浅色主题、多语言（i18n）
- [ ] CSP 硬化与安装包代码签名
- [ ] 远程管理、多设备统一监控
- [ ] 插件生态、脚本分享

## 设计原则

- **安全第一**：危险操作必须二次确认。
- **可解释性**：执行前说明原因、风险、后果。
- **不夸大**：不做「一键提速 300%」这类无法验证的承诺；口径说不清的数字，宁可不显示。
- **可追溯性**：所有操作可查看日志与回滚建议。
- **扩展性**：以插件与脚本体系持续演进。

## 项目愿景

Win-Top 的目标是成为 Windows 平台上专业、智能、可扩展的资源管理与系统维护工具。它不仅是一款监控工具，更是一个"系统专家"，帮助用户理解电脑、管理电脑、优化电脑。

---

## English

**Win-Top is a Windows system monitor and resource manager** built with Tauri, Rust and Svelte.

It unifies process management (**including per-process GPU usage and VRAM**), port-to-process mapping, disk space analysis, network throughput, startup-item control and AI-assisted diagnosis into one desktop workbench. Everything is collected straight from native Windows APIs (PDH / ETW / WMI / WinAPI) — **it never spawns `netstat`, `tasklist` or `powershell`** — and runs unelevated by default.

Highlights: per-process GPU utilization and dedicated/shared VRAM, a per-adapter GPU card, history trends persisted to disk (1 s raw for 7 days, 1 min rollups for 90 days, 200 MiB hard cap), NTFS MFT based whole-volume space analysis, and ETW-driven real-time process and per-process network events.

Requirements: Windows 10 1809+ / Windows 11, x64, WebView2 runtime. See [Releases](https://github.com/nickshui/Win-Top/releases).
