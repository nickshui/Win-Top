//! 每进程磁盘 I/O 追踪：使用 GetProcessIoCounters 获取每个进程的读写字节数，
//! 通过差分计算读写速率。不需要管理员权限。

use std::collections::HashMap;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use windows::Win32::Foundation::{CloseHandle, BOOL};
use windows::Win32::System::Threading::{
    GetProcessIoCounters, OpenProcess, IO_COUNTERS, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

#[derive(Serialize, Clone)]
pub struct DiskIoRow {
    pub pid: u32,
    pub name: String,
    pub read_bps: f64,
    pub write_bps: f64,
}

#[derive(Serialize, Clone)]
pub struct DiskIoSnapshot {
    pub ts: String,
    pub rows: Vec<DiskIoRow>,
}

/// Start the background tracking thread. Emits "disk-io" events.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut prev: HashMap<u32, (u64, u64)> = HashMap::new();
        let mut prev_time = Instant::now();

        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let now = Instant::now();
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed < 0.5 {
                continue;
            }
            prev_time = now;

            // Get process names once per iteration
            let names = crate::process::pid_name_map();

            // Enumerate PIDs using the shared helper
            let pids = crate::process::list_pids();

            let mut cur: HashMap<u32, (u64, u64)> = HashMap::new();

            for &pid in &pids {
                if pid == 0 {
                    continue;
                }
                unsafe {
                    // OpenProcess with PROCESS_QUERY_INFORMATION is sufficient
                    if let Ok(h) = OpenProcess(
                        PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
                        BOOL(0),
                        pid,
                    ) {
                        let mut io = IO_COUNTERS::default();
                        if GetProcessIoCounters(h, &mut io).is_ok() {
                            cur.insert(pid, (io.ReadTransferCount, io.WriteTransferCount));
                        }
                        let _ = CloseHandle(h);
                    }
                }
            }

            // Compute rates
            let mut rows: Vec<DiskIoRow> = Vec::new();
            for (&pid, &(read, write)) in &cur {
                let (pread, pwrite) = prev.get(&pid).copied().unwrap_or((read, write));
                let read_bps = read.saturating_sub(pread) as f64 / elapsed;
                let write_bps = write.saturating_sub(pwrite) as f64 / elapsed;
                if read_bps > 0.0 || write_bps > 0.0 {
                    let name = names
                        .get(&pid)
                        .cloned()
                        .unwrap_or_else(|| format!("PID {}", pid));
                    rows.push(DiskIoRow {
                        pid,
                        name,
                        read_bps,
                        write_bps,
                    });
                }
            }

            prev = cur;

            // Sort by total I/O descending
            rows.sort_by(|a, b| {
                (b.read_bps + b.write_bps)
                    .partial_cmp(&(a.read_bps + a.write_bps))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            // Keep top 15
            rows.truncate(15);

            let snapshot = DiskIoSnapshot {
                ts: chrono::Local::now().format("%H:%M:%S").to_string(),
                rows,
            };

            if app.emit_all("disk-io", snapshot).is_err() {
                break;
            }
        }
    });
}
