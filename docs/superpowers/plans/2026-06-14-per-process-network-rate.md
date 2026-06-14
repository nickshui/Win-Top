# 每进程网络速率（ETW Kernel-Network）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在「网络与端口」视图实时显示每个进程的上传/下载速率与会话累计收发量。

**Architecture:** 新建独立 ETW 实时会话订阅 `Microsoft-Windows-Kernel-Network`，回调按 PID 累加收发字节；采样线程每秒算速率差并 `emit` 给前端；前端在网络视图渲染总速率行 + Top-3 卡片 + 完整可排序表，非提权时降级为「以管理员重启」提示卡。

**Tech Stack:** Rust + Tauri 1.5 + ferrisetw 1（ETW）；Svelte 4 前端。

**关键约束：** 需管理员权限（同现有 Kernel-Process ETW）。提权实例运行时无法替换 `win-top.exe`，用 `cargo check` 验证编译；部署新二进制前关闭所有 Win-Top 窗口。

---

## 文件结构

- 创建 `poc-native/src/bin/nettraffic.rs` — POC：实测确认 event_id 与字段名（唯一技术风险，先行验证）
- 修改 `poc-native/Cargo.toml` — 注册 POC bin
- 创建 `src-tauri/src/nettraffic.rs` — ETW 采集模块（纯速率函数 + ETW 会话 + 采样器 + 状态）
- 修改 `src-tauri/src/main.rs` — 声明模块、`setup` 启动、注册命令
- 修改 `src/lib/stores.js` — 新增 store + `startNetTraffic()`
- 修改 `src/App.svelte` — 根级调用 `startNetTraffic()`
- 修改 `src/lib/views/Network.svelte` — 新增「进程流量」区块

---

## Task 1: POC — 实测 Kernel-Network event_id 与字段名

**Files:**
- Create: `poc-native/src/bin/nettraffic.rs`
- Modify: `poc-native/Cargo.toml`（追加 `[[bin]]`）

- [ ] **Step 1: 注册 POC bin**

在 `poc-native/Cargo.toml` 末尾追加：

```toml
[[bin]]
name = "nettraffic"
path = "src/bin/nettraffic.rs"
```

- [ ] **Step 2: 写 POC 源码**

创建 `poc-native/src/bin/nettraffic.rs`：

```rust
//! Kernel-Network ETW POC：确认 per-进程网络事件的 event_id 与字段名。
//! 需以管理员运行。采集 ~12 秒，统计各 event_id 计数，并对样本事件
//! 尝试多个候选字段名提取 PID / 字节数，结果写入 nettraffic-poc.txt。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::UserTrace;
use ferrisetw::EventRecord;

const KERNEL_NETWORK_GUID: &str = "7DD42A49-5329-4832-8DFD-43D979153A88";

static COUNTS: LazyLock<Mutex<HashMap<u16, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SAMPLES: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn probe(record: &EventRecord, locator: &SchemaLocator) {
    let schema = match locator.event_schema(record) {
        Ok(s) => s,
        Err(_) => return,
    };
    let id = record.event_id();
    {
        let mut c = COUNTS.lock().unwrap();
        *c.entry(id).or_insert(0) += 1;
    }
    let mut samples = SAMPLES.lock().unwrap();
    if samples.len() < 40 {
        let parser = Parser::create(record, &schema);
        let pid = ["PID", "Pid", "ProcessId", "ProcessID"]
            .iter()
            .find_map(|k| parser.try_parse::<u32>(k).ok().map(|v| format!("{}={}", k, v)))
            .unwrap_or_else(|| "PID=<none>".into());
        let size = ["size", "Size", "Length", "NumBytes"]
            .iter()
            .find_map(|k| parser.try_parse::<u32>(k).ok().map(|v| format!("{}={}", k, v)))
            .unwrap_or_else(|| "size=<none>".into());
        samples.push(format!("id={:<3} {} {}", id, pid, size));
    }
}

fn main() {
    let provider = Provider::by_guid(KERNEL_NETWORK_GUID)
        .add_callback(probe)
        .build();

    let trace = UserTrace::new()
        .named("WinTopNetPoc".to_string())
        .enable(provider)
        .start_and_process();

    let mut out = String::new();
    match trace {
        Ok(_t) => {
            println!("ETW 会话已启动，采集 12 秒（请同时下载文件/看视频制造流量）…");
            std::thread::sleep(Duration::from_secs(12));
            out.push_str("=== event_id 计数 ===\n");
            let counts = COUNTS.lock().unwrap();
            let mut ids: Vec<_> = counts.iter().collect();
            ids.sort_by_key(|(k, _)| **k);
            for (id, n) in ids {
                out.push_str(&format!("id={:<3} count={}\n", id, n));
            }
            out.push_str("\n=== 样本字段探测 ===\n");
            for s in SAMPLES.lock().unwrap().iter() {
                out.push_str(s);
                out.push('\n');
            }
        }
        Err(e) => {
            out.push_str(&format!("ETW 会话启动失败（多半未提权）：{:?}\n", e));
        }
    }
    let _ = std::fs::write("nettraffic-poc.txt", &out);
    println!("{out}");
}
```

- [ ] **Step 3: 编译 POC**

Run: `cd poc-native && cargo build --bin nettraffic`
Expected: `Finished`（编译通过即证明 ferrisetw API 用法正确）

- [ ] **Step 4: 以管理员运行 POC 并制造流量**

打开一个**管理员**终端（PowerShell/CMD「以管理员身份运行」），执行：
```
cd <repo>\poc-native
cargo run --bin nettraffic
```
运行的 12 秒内，在浏览器下载一个大文件或播放在线视频以产生流量。

- [ ] **Step 5: 核对实测结果**

Read: `poc-native/nettraffic-poc.txt`
Expected: `event_id 计数` 中出现非零计数的 id（预期发送 10/26/42/58、接收 11/27/43/59 这一组中的若干个）；`样本字段探测` 每行应是 `PID=<数字> size=<数字>`（而非 `<none>`）。

**核对要点（写入 Task 3 的依据）：**
- 若 PID 字段名不是 `PID`（如 `Pid`），记下实测名，Task 3 回调里的 `try_parse("PID")` 改为实测名。
- 若字节字段名不是 `size`，同理修正 Task 3 的 `try_parse("size")`。
- 若发送/接收的 event_id 与上面预期不符，以实测为准修正 Task 3 的 `matches!(... 10 | 42 | 26 | 58)`（发送）与 `11 | 43 | 27 | 59`（接收）。

- [ ] **Step 6: Commit**

```bash
git add poc-native/Cargo.toml poc-native/src/bin/nettraffic.rs
git commit -m "poc: 验证 Kernel-Network ETW event_id 与字段名"
```

---

## Task 2: 纯速率函数 diff_rates + 单元测试（TDD）

**Files:**
- Create: `src-tauri/src/nettraffic.rs`
- Modify: `src-tauri/src/main.rs`（声明模块）

**前置：** 确保仓库根存在 `dist/`（`cargo test` 会编译含 `generate_context!` 的 main.rs）。若没有，先在仓库根运行 `npm run build`。

- [ ] **Step 1: 声明模块**

在 `src-tauri/src/main.rs` 的 `#[cfg(target_os = "windows")] mod process;`（第 18 行附近）之后追加一行：

```rust
#[cfg(target_os = "windows")]
mod nettraffic;
```

- [ ] **Step 2: 写测试 + stub 实现**

创建 `src-tauri/src/nettraffic.rs`：

```rust
//! 每进程网络流量（EventTier）：订阅 Microsoft-Windows-Kernel-Network，
//! 按 PID 累加收发字节，采样线程每秒算速率并 emit。需要管理员权限。

use std::collections::HashMap;

/// 纯函数：由上一轮/本轮累计字节快照(pid->(sent,recv))与时间差，算每进程速率。
/// down = recv 增量 / elapsed，up = sent 增量 / elapsed。
/// 新 PID（prev 缺失）以 cur 作基线 → 速率 0。
/// 仅产出累计>0 的进程行。返回 (total_down_bps, total_up_bps, rows)，
/// rows 元素 = (pid, down_bps, up_bps, sent_total, recv_total)。
fn diff_rates(
    prev: &HashMap<u32, (u64, u64)>,
    cur: &HashMap<u32, (u64, u64)>,
    elapsed: f64,
) -> (f64, f64, Vec<(u32, f64, f64, u64, u64)>) {
    let _ = (prev, cur, elapsed);
    (0.0, 0.0, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::diff_rates;
    use std::collections::HashMap;

    #[test]
    fn computes_per_pid_rates_and_totals() {
        let mut prev = HashMap::new();
        prev.insert(100u32, (1_000u64, 2_000u64)); // (sent, recv)
        let mut cur = HashMap::new();
        cur.insert(100u32, (1_500u64, 6_000u64)); // +500 sent, +4000 recv，跨 2 秒

        let (down, up, rows) = diff_rates(&prev, &cur, 2.0);
        assert_eq!(rows.len(), 1);
        let (pid, d, u, sent, recv) = rows[0];
        assert_eq!(pid, 100);
        assert!((d - 2000.0).abs() < 1e-6, "down={d}"); // 4000/2
        assert!((u - 250.0).abs() < 1e-6, "up={u}"); // 500/2
        assert_eq!(sent, 1500);
        assert_eq!(recv, 6000);
        assert!((down - 2000.0).abs() < 1e-6);
        assert!((up - 250.0).abs() < 1e-6);
    }

    #[test]
    fn new_pid_uses_baseline_zero_rate() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(7u32, (5_000u64, 5_000u64));
        let (_d, _u, rows) = diff_rates(&prev, &cur, 1.0);
        assert_eq!(rows.len(), 1); // 累计>0 仍产出行
        assert_eq!(rows[0].1, 0.0); // down 速率 0
        assert_eq!(rows[0].2, 0.0); // up 速率 0
    }

    #[test]
    fn zero_elapsed_yields_nothing() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(1u32, (10, 10));
        let (d, u, rows) = diff_rates(&prev, &cur, 0.0);
        assert_eq!(d, 0.0);
        assert_eq!(u, 0.0);
        assert!(rows.is_empty());
    }
}
```

- [ ] **Step 3: 运行测试确认失败**

Run: `cd src-tauri && cargo test nettraffic`
Expected: 编译通过，3 个测试中 `computes_per_pid_rates_and_totals` 与 `new_pid_uses_baseline_zero_rate` FAIL（stub 返回空）。

- [ ] **Step 4: 实现 diff_rates**

把 `src-tauri/src/nettraffic.rs` 中的 `diff_rates` 函数体替换为：

```rust
fn diff_rates(
    prev: &HashMap<u32, (u64, u64)>,
    cur: &HashMap<u32, (u64, u64)>,
    elapsed: f64,
) -> (f64, f64, Vec<(u32, f64, f64, u64, u64)>) {
    let mut total_down = 0.0;
    let mut total_up = 0.0;
    let mut rows = Vec::new();
    if elapsed <= 0.0 {
        return (0.0, 0.0, rows);
    }
    for (&pid, &(sent, recv)) in cur {
        let (psent, precv) = prev.get(&pid).copied().unwrap_or((sent, recv));
        let up = sent.saturating_sub(psent) as f64 / elapsed;
        let down = recv.saturating_sub(precv) as f64 / elapsed;
        total_up += up;
        total_down += down;
        if sent + recv > 0 {
            rows.push((pid, down, up, sent, recv));
        }
    }
    (total_down, total_up, rows)
}
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cd src-tauri && cargo test nettraffic`
Expected: `test result: ok. 3 passed`

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/nettraffic.rs src-tauri/src/main.rs
git commit -m "feat(nettraffic): 纯速率差函数 diff_rates + 单元测试"
```

---

## Task 3: ETW 会话 + 回调 + 采样器 + 状态

**Files:**
- Modify: `src-tauri/src/nettraffic.rs`（在 diff_rates 基础上补齐 IO 外壳）

**说明：** ETW 回调与采样循环无法单元测试，本任务以 `cargo check` 验证编译，功能在 Task 7 实测。回调里的字段名/event_id 以 **Task 1 实测结果** 为准（下面用预期值，若 POC 不同则替换）。

- [ ] **Step 1: 用完整版替换 nettraffic.rs**

将 `src-tauri/src/nettraffic.rs` 整个文件替换为以下内容（保留了 Task 2 的 diff_rates 与测试）：

```rust
//! 每进程网络流量（EventTier）：订阅 Microsoft-Windows-Kernel-Network，
//! 按 PID 累加收发字节，采样线程每秒算速率并 emit。需要管理员权限。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, UserTrace};
use ferrisetw::EventRecord;

const KERNEL_NETWORK_GUID: &str = "7DD42A49-5329-4832-8DFD-43D979153A88";
const SESSION_NAME: &str = "WinTopNetTrace";

// pid -> (sent, recv) 累计字节，自会话启动以来。
static ACCUM: LazyLock<Mutex<HashMap<u32, (u64, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NET_OK: AtomicBool = AtomicBool::new(false);
static NET_REASON: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new("初始化中…".to_string()));

#[derive(Serialize, Clone)]
pub struct NetTrafficStatus {
    pub available: bool,
    pub reason: String,
}

pub fn status() -> NetTrafficStatus {
    NetTrafficStatus {
        available: NET_OK.load(Ordering::Relaxed),
        reason: NET_REASON.lock().map(|s| s.clone()).unwrap_or_default(),
    }
}

fn set_status(app: &AppHandle, available: bool, reason: &str) {
    NET_OK.store(available, Ordering::Relaxed);
    if let Ok(mut r) = NET_REASON.lock() {
        *r = reason.to_string();
    }
    let _ = app.emit_all(
        "net-traffic-status",
        NetTrafficStatus {
            available,
            reason: reason.to_string(),
        },
    );
}

#[derive(Serialize, Clone)]
struct TrafficRow {
    pid: u32,
    name: String,
    down_bps: f64,
    up_bps: f64,
    sent_total: u64,
    recv_total: u64,
}

#[derive(Serialize, Clone)]
struct TrafficSnapshot {
    ts: String,
    total_down_bps: f64,
    total_up_bps: f64,
    rows: Vec<TrafficRow>,
}

/// 纯函数：由上一轮/本轮累计字节快照(pid->(sent,recv))与时间差，算每进程速率。
/// down = recv 增量 / elapsed，up = sent 增量 / elapsed。
/// 新 PID（prev 缺失）以 cur 作基线 → 速率 0。
/// 仅产出累计>0 的进程行。返回 (total_down_bps, total_up_bps, rows)，
/// rows 元素 = (pid, down_bps, up_bps, sent_total, recv_total)。
fn diff_rates(
    prev: &HashMap<u32, (u64, u64)>,
    cur: &HashMap<u32, (u64, u64)>,
    elapsed: f64,
) -> (f64, f64, Vec<(u32, f64, f64, u64, u64)>) {
    let mut total_down = 0.0;
    let mut total_up = 0.0;
    let mut rows = Vec::new();
    if elapsed <= 0.0 {
        return (0.0, 0.0, rows);
    }
    for (&pid, &(sent, recv)) in cur {
        let (psent, precv) = prev.get(&pid).copied().unwrap_or((sent, recv));
        let up = sent.saturating_sub(psent) as f64 / elapsed;
        let down = recv.saturating_sub(precv) as f64 / elapsed;
        total_up += up;
        total_down += down;
        if sent + recv > 0 {
            rows.push((pid, down, up, sent, recv));
        }
    }
    (total_down, total_up, rows)
}

/// 在后台线程启动 ETW 会话 + 采样循环。未提权会失败并报告状态。
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let provider = Provider::by_guid(KERNEL_NETWORK_GUID)
            .add_callback(|record: &EventRecord, locator: &SchemaLocator| {
                let schema = match locator.event_schema(record) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                // 方向：发送 10/26/42/58，接收 11/27/43/59（以 Task 1 POC 实测为准）
                let is_sent = matches!(record.event_id(), 10 | 26 | 42 | 58);
                let is_recv = matches!(record.event_id(), 11 | 27 | 43 | 59);
                if !is_sent && !is_recv {
                    return;
                }
                let parser = Parser::create(record, &schema);
                let pid: u32 = match parser.try_parse("PID") {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let size: u32 = parser.try_parse("size").unwrap_or(0);
                if size == 0 {
                    return;
                }
                if let Ok(mut acc) = ACCUM.lock() {
                    let e = acc.entry(pid).or_insert((0, 0));
                    if is_sent {
                        e.0 += size as u64;
                    } else {
                        e.1 += size as u64;
                    }
                }
            })
            .build();

        // 防会话泄漏：固定名 + 启动前回收（与 events.rs 同坑）。
        let _ = stop_trace_by_name(SESSION_NAME);

        match UserTrace::new()
            .named(SESSION_NAME.to_string())
            .enable(provider)
            .start_and_process()
        {
            Ok(_trace) => {
                set_status(&app, true, "");
                run_sampler(app); // 持续运行，保持 _trace 存活（drop 会停会话）
            }
            Err(e) => {
                set_status(&app, false, &format!("网络流量 ETW 会话启动失败：{:?}", e));
            }
        }
    });
}

fn run_sampler(app: AppHandle) {
    let mut prev: HashMap<u32, (u64, u64)> = HashMap::new();
    let mut last = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = Instant::now();
        let elapsed = now.duration_since(last).as_secs_f64();
        last = now;

        let cur = ACCUM.lock().map(|a| a.clone()).unwrap_or_default();
        let (total_down, total_up, raw_rows) = diff_rates(&prev, &cur, elapsed);

        let names = crate::process::pid_name_map();
        let mut rows: Vec<TrafficRow> = raw_rows
            .into_iter()
            .map(|(pid, down, up, sent, recv)| TrafficRow {
                pid,
                name: names.get(&pid).cloned().unwrap_or_else(|| {
                    if pid == 0 {
                        "System Idle".to_string()
                    } else {
                        format!("PID {}", pid)
                    }
                }),
                down_bps: down,
                up_bps: up,
                sent_total: sent,
                recv_total: recv,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.down_bps
                .partial_cmp(&a.down_bps)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 剪枝：丢弃已退出进程，防 ACCUM 无限增长。
        if let Ok(mut acc) = ACCUM.lock() {
            acc.retain(|pid, _| names.contains_key(pid));
        }
        prev = cur;

        let snapshot = TrafficSnapshot {
            ts: Local::now().format("%H:%M:%S").to_string(),
            total_down_bps: total_down,
            total_up_bps: total_up,
            rows,
        };
        if app.emit_all("net-traffic", snapshot).is_err() {
            break; // 窗口已关闭
        }
    }
}

#[cfg(test)]
mod tests {
    use super::diff_rates;
    use std::collections::HashMap;

    #[test]
    fn computes_per_pid_rates_and_totals() {
        let mut prev = HashMap::new();
        prev.insert(100u32, (1_000u64, 2_000u64)); // (sent, recv)
        let mut cur = HashMap::new();
        cur.insert(100u32, (1_500u64, 6_000u64)); // +500 sent, +4000 recv，跨 2 秒

        let (down, up, rows) = diff_rates(&prev, &cur, 2.0);
        assert_eq!(rows.len(), 1);
        let (pid, d, u, sent, recv) = rows[0];
        assert_eq!(pid, 100);
        assert!((d - 2000.0).abs() < 1e-6, "down={d}");
        assert!((u - 250.0).abs() < 1e-6, "up={u}");
        assert_eq!(sent, 1500);
        assert_eq!(recv, 6000);
        assert!((down - 2000.0).abs() < 1e-6);
        assert!((up - 250.0).abs() < 1e-6);
    }

    #[test]
    fn new_pid_uses_baseline_zero_rate() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(7u32, (5_000u64, 5_000u64));
        let (_d, _u, rows) = diff_rates(&prev, &cur, 1.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 0.0);
        assert_eq!(rows[0].2, 0.0);
    }

    #[test]
    fn zero_elapsed_yields_nothing() {
        let prev = HashMap::new();
        let mut cur = HashMap::new();
        cur.insert(1u32, (10, 10));
        let (d, u, rows) = diff_rates(&prev, &cur, 0.0);
        assert_eq!(d, 0.0);
        assert_eq!(u, 0.0);
        assert!(rows.is_empty());
    }
}
```

- [ ] **Step 2: 编译检查**

Run: `cd src-tauri && cargo check`
Expected: `Finished`（可能有 `TrafficRow`/`status` 等暂未被外部使用的 dead_code 警告，Task 4 接线后消除）。

- [ ] **Step 3: 单元测试仍通过**

Run: `cd src-tauri && cargo test nettraffic`
Expected: `test result: ok. 3 passed`

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/nettraffic.rs
git commit -m "feat(nettraffic): Kernel-Network ETW 会话 + 采样器 + 状态上报"
```

---

## Task 4: main.rs 接线（setup 启动 + 注册命令）

**Files:**
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 1: 加命令 get_nettraffic_status**

在 `src-tauri/src/main.rs` 的 `get_etw_status` 命令定义（`fn get_etw_status()` 那块，第 89-93 行附近）之后追加：

```rust
#[cfg(target_os = "windows")]
#[tauri::command]
fn get_nettraffic_status() -> nettraffic::NetTrafficStatus {
    nettraffic::status()
}
```

- [ ] **Step 2: setup 里启动采集**

在 `setup` 闭包中，把这一行：

```rust
            // EventTier：ETW 实时进程事件（未提权会自行报告 etw-status=false）
            events::start(handle);
```

替换为（先克隆给 nettraffic，再把 handle 交给 events）：

```rust
            // NetEventTier：ETW 每进程网络流量（未提权会报告 net-traffic-status=false）
            nettraffic::start(handle.clone());
            // EventTier：ETW 实时进程事件（未提权会自行报告 etw-status=false）
            events::start(handle);
```

- [ ] **Step 3: 注册到 invoke_handler**

在 `tauri::generate_handler![ ... ]` 列表中，把 `get_etw_status` 那一行改为带逗号并追加新命令：

```rust
            get_etw_status,
            get_nettraffic_status
```

- [ ] **Step 4: 编译检查**

Run: `cd src-tauri && cargo check`
Expected: `Finished`，且 Task 3 的 dead_code 警告消失（`status`/`start` 现已被引用）。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/main.rs
git commit -m "feat(nettraffic): main 接线 setup 启动 + get_nettraffic_status 命令"
```

---

## Task 5: 前端 stores + App 根级订阅

**Files:**
- Modify: `src/lib/stores.js`
- Modify: `src/App.svelte`

- [ ] **Step 1: stores.js 新增流量 store 与订阅**

在 `src/lib/stores.js` 末尾（`relaunchAdmin` 之后）追加：

```js
// 每进程网络流量（来自 ETW Kernel-Network）
export const netTraffic = writable(null);
export const netTrafficAvailable = writable(false);
export const netTrafficReason = writable("");

function refreshNetTrafficStatus() {
  invoke("get_nettraffic_status")
    .then((s) => {
      netTrafficAvailable.set(!!s.available);
      netTrafficReason.set(s.reason || "");
    })
    .catch(() => {});
}

export function startNetTraffic() {
  let un1, un2;
  listen("net-traffic", (e) => netTraffic.set(e.payload)).then((u) => (un1 = u));
  listen("net-traffic-status", (e) => {
    netTrafficAvailable.set(!!e.payload.available);
    netTrafficReason.set(e.payload.reason || "");
  }).then((u) => (un2 = u));
  // 主动查询当前状态（规避后端先于监听器 emit 的时序竞争），并稍后重查一次
  refreshNetTrafficStatus();
  setTimeout(refreshNetTrafficStatus, 1500);
  return () => {
    if (un1) un1();
    if (un2) un2();
  };
}
```

- [ ] **Step 2: App.svelte 根级调用**

在 `src/App.svelte` 的 import 行：

```js
  import { startMetrics, startEvents } from "./lib/stores.js";
```

改为：

```js
  import { startMetrics, startEvents, startNetTraffic } from "./lib/stores.js";
```

在 `<script>` 顶部变量声明处，`let stopEvents;` 之后追加：

```js
  let stopNetTraffic;
```

把 `onMount` 改为：

```js
  onMount(() => {
    stopMetrics = startMetrics();
    stopEvents = startEvents();
    stopNetTraffic = startNetTraffic();
  });
```

把 `onDestroy` 改为：

```js
  onDestroy(() => {
    if (stopMetrics) stopMetrics();
    if (stopEvents) stopEvents();
    if (stopNetTraffic) stopNetTraffic();
  });
```

- [ ] **Step 3: 构建检查**

Run: `npm run build`（仓库根）
Expected: `built in ...`，无 Svelte 编译错误。

- [ ] **Step 4: Commit**

```bash
git add src/lib/stores.js src/App.svelte
git commit -m "feat(nettraffic): 前端 netTraffic store + 根级订阅"
```

---

## Task 6: Network.svelte 「进程流量」区块

**Files:**
- Modify: `src/lib/views/Network.svelte`

复用本组件已有的 `table`/`thead th`/`td`/`.num`/`.mono`/`.empty`/`.primary`/`.section-title`/`.nt-head`/`.ck-hint`/`.table-wrap` 样式，只新增少量类。

- [ ] **Step 1: 引入 store 与格式化助手**

在 `<script>` 顶部，把：

```js
  import { pushToast } from "../stores.js";
```

改为：

```js
  import { pushToast, netTraffic, netTrafficAvailable, relaunchAdmin } from "../stores.js";
```

在 `<script>` 内（任意已有变量声明之后，例如 `let timer;` 附近）追加：

```js
  // 进程流量表：本地排序状态 + 格式化
  let tSortKey = "down_bps";
  let tSortDir = -1; // -1 降序, 1 升序
  function setTSort(k) {
    if (tSortKey === k) tSortDir = -tSortDir;
    else {
      tSortKey = k;
      tSortDir = k === "name" ? 1 : -1;
    }
  }
  const tArrow = (k) => (tSortKey === k ? (tSortDir === -1 ? " ↓" : " ↑") : "");

  const fmtRate = (bps) => {
    if (!bps || bps < 1) return "0 B/s";
    if (bps < 1024) return `${bps.toFixed(0)} B/s`;
    if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
    return `${(bps / 1024 / 1024).toFixed(2)} MB/s`;
  };
  const fmtBytes = (n) => {
    if (!n) return "0";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };

  $: traffic = $netTraffic;
  $: trafficRows = (traffic?.rows ?? [])
    .slice()
    .sort((a, b) => {
      if (tSortKey === "name") return tSortDir * a.name.localeCompare(b.name);
      return tSortDir * (a[tSortKey] - b[tSortKey]);
    });
  $: top3 = (traffic?.rows ?? [])
    .slice()
    .sort((a, b) => b.down_bps - a.down_bps)
    .slice(0, 3);
```

- [ ] **Step 2: 插入「进程流量」标记**

在 `</section>`（网络工具区块结束，第 247 行附近）与 `<div class="toolbar">`（端口表工具栏，第 249 行附近）之间，插入：

```svelte
<section class="traffic">
  <header class="nt-head">
    <h2 class="section-title">进程流量 · 实时</h2>
    {#if $netTrafficAvailable && traffic}
      <span class="total mono">↓ {fmtRate(traffic.total_down_bps)}　↑ {fmtRate(traffic.total_up_bps)}</span>
    {/if}
  </header>

  {#if !$netTrafficAvailable}
    <div class="gate">
      <p>每进程网络流量监测基于实时 ETW，需要<strong>管理员权限</strong>。</p>
      <button class="primary" on:click={relaunchAdmin}>以管理员重启</button>
    </div>
  {:else if !traffic}
    <p class="ck-hint">正在采集流量数据…</p>
  {:else}
    <div class="top3">
      {#each top3 as t (t.pid)}
        <div class="tcard">
          <div class="tname" title={t.name}>{t.name}</div>
          <div class="tdown mono">↓ {fmtRate(t.down_bps)}</div>
          <div class="tup mono">↑ {fmtRate(t.up_bps)}</div>
        </div>
      {/each}
      {#if top3.length === 0}
        <div class="tcard empty-card">暂无流量</div>
      {/if}
    </div>

    <div class="table-wrap traffic-table">
      <table>
        <thead>
          <tr>
            <th class="col-proc" on:click={() => setTSort("name")}>进程{tArrow("name")}</th>
            <th class="num" on:click={() => setTSort("pid")}>PID{tArrow("pid")}</th>
            <th class="num" on:click={() => setTSort("down_bps")}>↓ 下载{tArrow("down_bps")}</th>
            <th class="num" on:click={() => setTSort("up_bps")}>↑ 上传{tArrow("up_bps")}</th>
            <th class="num" on:click={() => setTSort("recv_total")}>累计↓{tArrow("recv_total")}</th>
            <th class="num" on:click={() => setTSort("sent_total")}>累计↑{tArrow("sent_total")}</th>
          </tr>
        </thead>
        <tbody>
          {#if trafficRows.length === 0}
            <tr><td colspan="6" class="empty">暂无有流量的进程</td></tr>
          {:else}
            {#each trafficRows as t (t.pid)}
              <tr>
                <td class="col-proc" title={t.name}>{t.name}</td>
                <td class="num mono">{t.pid}</td>
                <td class="num mono">{fmtRate(t.down_bps)}</td>
                <td class="num mono">{fmtRate(t.up_bps)}</td>
                <td class="num mono">{fmtBytes(t.recv_total)}</td>
                <td class="num mono">{fmtBytes(t.sent_total)}</td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    </div>
  {/if}
</section>
```

- [ ] **Step 3: 追加样式**

在 `<style>` 块末尾（最后一个 `}` 与 `</style>` 之间）追加：

```css
  .traffic {
    margin-bottom: var(--sp-6);
    padding: var(--sp-4) var(--sp-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .traffic .total {
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .traffic thead th {
    cursor: pointer;
    user-select: none;
  }
  .traffic thead th:hover {
    color: var(--text);
  }
  .traffic-table tbody {
    max-height: 300px;
  }
  .gate {
    margin-top: var(--sp-3);
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--text-muted);
  }
  .top3 {
    margin: var(--sp-4) 0;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--sp-3);
  }
  .tcard {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--sp-3) var(--sp-4);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tname {
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tdown {
    font-size: 20px;
    font-weight: 700;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }
  .tup {
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .empty-card {
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
  }
```

- [ ] **Step 4: 构建检查**

Run: `npm run build`（仓库根）
Expected: `built in ...`，无 Svelte 编译错误。

- [ ] **Step 5: Commit**

```bash
git add src/lib/views/Network.svelte
git commit -m "feat(nettraffic): 网络视图进程流量区块(总速率/Top3/可排序表)"
```

---

## Task 7: 提权功能实测（对照任务管理器）

**Files:** 无（端到端验证）。

- [ ] **Step 1: 关闭所有 Win-Top 窗口**

确保没有残留的提权实例占用 `win-top.exe`，否则下一步构建会「拒绝访问」。如端口 1420 残留，用管理员 PowerShell：`Get-NetTCPConnection -LocalPort 1420 | Select OwningProcess` 找到并结束。

- [ ] **Step 2: 以管理员启动 dev**

在**管理员**终端，仓库根运行：`npm run tauri dev`
打开后切到「网络与端口」视图。

- [ ] **Step 3: 验证降级路径（可选）**

若以**普通**权限启动，「进程流量」区块应显示「需要管理员权限」+「以管理员重启」按钮，无报错。

- [ ] **Step 4: 验证数据正确性**

提权实例中，浏览器下载大文件或播放在线视频。预期：
- 顶部总速率 `↓/↑` 实时跳动；
- Top-3 卡片出现 chrome/浏览器等进程；
- 完整表里对应进程的 ↓/↑ 速率量级与**任务管理器→进程→网络列**一致（同数量级即可，非逐字节相等）；
- 「累计↓/累计↑」随时间单调增长。

- [ ] **Step 5: 用户截图确认 GUI**

WebView2 无法 attach 截图工具，请用户对「网络与端口」视图截图确认渲染（沿用项目惯例）。

- [ ] **Step 6: 标记完成**

无新代码改动则无需 commit；若实测中按 Task 1 核对修正了 event_id/字段名，单独 commit 修正。

---

## 自检小结（写计划时已核对）

- **Spec 覆盖：** 数据源选型→Task 1；后端模块/累加/采样/剪枝/状态→Task 2-3；main 接线→Task 4；stores→Task 5；前端总速率行/Top-3/完整表/提权门禁/格式化→Task 6；POC 与功能验证→Task 1/7。`total_*` 字段已在 emit 结构中预留。
- **无占位符：** 所有步骤含完整代码与确切命令。
- **类型一致：** `diff_rates` 签名（Task 2/3 一致）、`NetTrafficStatus`（Task 3 定义、Task 4 引用、Task 5 消费 `available`/`reason`）、emit 字段 `total_down_bps`/`total_up_bps`/`rows[{pid,name,down_bps,up_bps,sent_total,recv_total}]`（Task 3 产出、Task 6 消费）逐项对齐。

