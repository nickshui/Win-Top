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
