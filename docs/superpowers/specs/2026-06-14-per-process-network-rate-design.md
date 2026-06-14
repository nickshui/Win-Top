# 设计：每进程网络上传/下载速率（ETW Kernel-Network）

日期：2026-06-14
状态：已确认，待写实现计划

## 目标

在「网络与端口」视图中，实时显示**每个进程**的网络上传/下载速率与会话累计收发量。
现有端口表是「按连接」枚举（netstat2），无法给出速率；本特性新增一条「按进程」的实时流量数据流。

## 背景与约束

- Windows 没有简单的非管理员 API 能拿到某进程的实时收发字节。任务管理器自身也依赖 ETW。
- 项目已有一套 ETW 实践（`events.rs` 订阅 `Microsoft-Windows-Kernel-Process`，`ferrisetw::UserTrace` + 固定会话名防泄漏 + 提权门禁）。本特性原样复刻该模式。
- **本特性需要管理员权限**，与现有实时事件特性一致。非提权时优雅降级为提示卡 +「以管理员重启」。

## 数据源选型

| 方案 | 覆盖 | 权限 | 结论 |
|---|---|---|---|
| **ETW `Microsoft-Windows-Kernel-Network`** | TCP+UDP 收发，事件带 PID + 字节数 | 管理员 | **采用**：准确、实时、复用现有 ferrisetw 框架 |
| `GetPerTcpConnectionEStats` | 仅 TCP（无 UDP/QUIC） | 管理员 | 否决：需逐连接启用 ESTATS，代码量大、覆盖差 |
| `GetProcessIoCounters` | 磁盘+网络+IPC 混合 | 普通 | 否决：无法隔离网络流量 |

## 架构与数据流

```
ETW Kernel-Network 会话(WinTopNetTrace)
  └─ 回调: 按 event_id 判方向, 累加 size 到 ACCUM[pid]=(sent,recv)   ← 廉价, 不逐事件 emit
采样线程(每 1s)
  └─ 快照 ACCUM → 与上次差值算速率 → pid_name_map() 解析名 → emit "net-traffic"
前端 stores.startNetTraffic()
  └─ 监听 "net-traffic" / "net-traffic-status" → 写 store
Network.svelte
  └─ 全局总速率行 + Top-3 卡片 + 完整进程流量表 (提权门禁)
```

## 后端设计

### 新模块 `src-tauri/src/nettraffic.rs`（镜像 `events.rs`）

- 独立 ETW 实时会话，固定名 `WinTopNetTrace`；`start_and_process()` 前先 `stop_trace_by_name(SESSION_NAME)` 回收同名残留会话（防止反复重启导致的会话泄漏，与 `events.rs` 同一坑）。
- 订阅 provider GUID `7DD42A49-5329-4832-8DFD-43D979153A88`（`Microsoft-Windows-Kernel-Network`）。
- 累加器：`static ACCUM: LazyLock<Mutex<HashMap<u32,(u64 /*sent*/, u64 /*recv*/)>>>`，存自会话启动以来的累计字节。
- 回调按 `record.event_id()` 判方向并累加 `size` 字段：

  | 方向 | TCP IPv4 | TCP IPv6 | UDP IPv4 | UDP IPv6 |
  |---|---|---|---|---|
  | 发送(sent) | 10 | 42 | 26 | 58 |
  | 接收(recv) | 11 | 43 | 27 | 59 |

  字段提取：`parser.try_parse::<u32>("PID")`、`parser.try_parse::<u32>("size")`。
- **技术风险点（唯一）**：上述 event_id 与字段名（`PID`/`size`）需在实现首步用 `poc-native/` 跑一个独立 POC bin 实测 dump 确认（与磁盘温度那次同样的先 POC 后落地策略）。若字段名不同（如 `Pid`/`Size`/`daddr`），以 POC 实测为准修正。

### 采样线程（每 1s）

- 快照当前 `ACCUM` → 与上一轮快照逐 PID 求差 → 得每进程 `down_bps`(recv 增量) / `up_bps`(sent 增量)，按 `1/elapsed` 归一为字节每秒。
- 用 `crate::process::pid_name_map()` 解析进程名（与网络端口模块一致；未知 PID 回退 `PID {n}` / `System Idle`）。
- 剪枝：每周期用 `pid_name_map()` 的键集合过滤 `ACCUM`，丢弃已退出进程的条目，防止 map 无限增长。
- 首轮无上一快照 → 速率记 0，仅建立基线。

### emit 数据结构（`net-traffic` 事件）

```jsonc
{
  "ts": "HH:MM:SS",
  "total_down_bps": 3_456_789.0,   // 所有进程下载速率之和(字节/秒)
  "total_up_bps": 234_567.0,
  "rows": [
    { "pid": 4821, "name": "chrome.exe",
      "down_bps": 3_200_000.0, "up_bps": 400_000.0,
      "sent_total": 92_274_688, "recv_total": 1_288_490_188 }
  ]
}
```
`total_*` 为将来「概览实时吞吐」spec 预留复用，本特性仅在网络视图消费。

### 状态上报（复刻 `events.rs` 的 `EtwStatusInfo` 模式）

- `static NET_OK: AtomicBool` + `static NET_REASON: Mutex<String>`。
- 会话启动成功 → emit `net-traffic-status {available:true, reason:""}`；失败（多为未提权）→ `{available:false, reason:"…"}`。
- 命令 `get_nettraffic_status() -> NetTrafficStatus`，规避「后端先于前端监听器 emit」的时序竞争（与现有 `get_etw_status` 一致）。

### `main.rs` 接线

- `setup` 中 `nettraffic::start(handle.clone())`（与 `events::start` 并列）。
- `invoke_handler` 注册 `get_nettraffic_status`。

## 前端设计

### `src/lib/stores.js`

- 新增 store：`netTraffic`（writable null）、`netTrafficAvailable`（bool）、`netTrafficReason`（string）。
- `startNetTraffic()`：监听 `net-traffic` 写 `netTraffic`；监听 `net-traffic-status` 与主动 `get_nettraffic_status()` 写可用性（同 `startEvents` 的双保险）。返回取消订阅函数。
- 在 App 根级调用一次（与 `startMetrics`/`startEvents` 并列），保证视图切换间数据持续。

### `src/lib/views/Network.svelte`（在「网络工具」区块与端口表之间插入「进程流量」区块）

- **全局总速率行**：`↓ {fmtRate(total_down_bps)}   ↑ {fmtRate(total_up_bps)}`。
- **Top-3 卡片**：对 `rows` 按 `down_bps` 降序取前 3，每卡显示进程名 + 大字 ↓ 速率 + 小字 ↑ 速率。
- **完整进程流量表**：列 = 进程 / PID / ↓ / ↑ / 会话累计↓ / 会话累计↑；可点表头排序（默认按 ↓ 降序），可搜索（按名称/PID，复用现有 search 样式）。仅展示有过流量的进程（`sent_total+recv_total>0`）。
- **提权门禁**：`netTrafficAvailable===false` 时整块替换为提示卡（文案：需管理员权限才能监测每进程流量）+「以管理员重启」按钮（复用 `stores.relaunchAdmin`）。
- **格式化助手**（本组件内）：
  - `fmtRate(bps)`：`<1KB/s` 显示 B/s，`<1MB/s` 显示 KB/s，否则 MB/s（1 位小数）。
  - `fmtBytes(n)`：会话累计，KB/MB/GB 自适应。

## 边界与安全

- **会话泄漏**：固定会话名 + 启动前 `stop_trace_by_name`（已在设计中）。
- **事件洪峰**：回调只做小 map 的 mutex 累加，emit 频率固定 1/秒（非逐事件），开销可控。
- **PID 复用**：累计按 PID 键；进程退出被剪枝后，若 PID 被新进程复用则累计从 0 重新计，可接受。
- **构建坑（沿用已知）**：提权实例运行时无法替换 `win-top.exe`，用 `cargo check` 验证编译；部署新二进制前需关闭所有 Win-Top 窗口。ETW/流量数据仅在提权实例可见。

## 验证策略

1. **POC**：`poc-native/` 新增 bin，启动 Kernel-Network 会话，dump 若干事件的 event_id + 可用字段名，确认 ID 表与 `PID`/`size` 字段（唯一技术风险，先于实现落地）。
2. **编译**：`cargo check`（提权实例在跑时不替换 exe）。
3. **功能**：提权运行 → 下载大文件 / 看视频 → 对比任务管理器「网络」列，速率量级应一致；累计随时间单调增长。
4. **降级**：非提权运行 → 看到提示卡与「以管理员重启」，无报错。
5. **GUI**：WebView2 无法 attach 截图工具，最终渲染由用户截图确认（沿用项目惯例）。

## 不在本特性范围（后续独立 spec）

- 概览 Bento 实时吞吐（本设计已备好 `total_*` 供复用）。
- 进程管理表的网络列。
- 连接级流量明细（点进程展开各远端）。
