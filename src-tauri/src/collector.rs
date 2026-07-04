//! 常驻采集器（FastTier）：持有长生命周期的 PDH 查询句柄，
//! 每秒采集一次 CPU + 内存，通过 Tauri 事件推送给前端。
//!
//! 与旧实现的区别：
//!   - PDH 句柄只打开一次，循环复用（旧实现每次 System::new_all 重建，开销巨大）
//!   - CPU 用 PDH English 计数器，规避中文系统本地化 + 修复 sysinfo 恒 0 问题
//!   - 推送式（emit）而非前端每秒 invoke 轮询

use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant};

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use windows::core::w;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
};
use windows::Win32::NetworkManagement::IpHelper::{GetIfTable, MIB_IFTABLE, MIB_IFROW};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[derive(Serialize, Clone)]
pub struct MetricsSnapshot {
    pub ts: String,
    pub cpu: f64,
    pub mem_load: u32,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub mem_page_total_gb: f64,
    pub mem_page_used_gb: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net_total_bps: f64,
}

fn read_memory() -> Option<(u32, f64, f64, f64, f64)> {
    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut ms).is_ok() {
            let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
            Some((
                ms.dwMemoryLoad,
                gb(ms.ullTotalPhys.saturating_sub(ms.ullAvailPhys)),
                gb(ms.ullTotalPhys),
                gb(ms.ullTotalPageFile),
                gb(ms.ullTotalPageFile.saturating_sub(ms.ullAvailPageFile)),
            ))
        } else {
            None
        }
    }
}

/// Read cumulative total network bytes (in+out) across all interfaces.
/// Returns (total_in, total_out) cumulative 64-bit values.
/// Uses GetIfTable — 32-bit counters but we sum into u64 per-interface.
unsafe fn read_net_bytes() -> Option<(u64, u64)> {
    let mut size: u32 = 0;
    // First call to get required buffer size (returns ERROR_INSUFFICIENT_BUFFER=122)
    let ret = GetIfTable(None, &mut size, false);
    if ret != 122 || size == 0 {
        return None;
    }
    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let table = buf.as_mut_ptr() as *mut MIB_IFTABLE;
    if GetIfTable(Some(table), &mut size, false) != 0 {
        return None;
    }
    let entries = (*table).dwNumEntries as usize;
    let rows_ptr = &(*table).table as *const MIB_IFROW;
    let rows = std::slice::from_raw_parts(rows_ptr, entries);
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    for row in rows {
        total_in += row.dwInOctets as u64;
        total_out += row.dwOutOctets as u64;
    }
    Some((total_in, total_out))
}

/// 在独立线程中运行。打开一次 PDH 查询，循环采集并 emit。
pub fn run_metrics_loop(app: AppHandle) {
    unsafe {
        let mut query: isize = 0;
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            eprintln!("[collector] PdhOpenQueryW 失败");
            return;
        }

        // CPU 计数器
        let mut counter_cpu: isize = 0;
        if PdhAddEnglishCounterW(
            query,
            w!("\\Processor(_Total)\\% Processor Time"),
            0,
            &mut counter_cpu,
        ) != 0
        {
            eprintln!("[collector] CPU 计数器添加失败");
            return;
        }

        // 磁盘读取字节/秒
        let mut counter_disk_r: isize = 0;
        let _ = PdhAddEnglishCounterW(
            query,
            w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec"),
            0,
            &mut counter_disk_r,
        );

        // 磁盘写入字节/秒
        let mut counter_disk_w: isize = 0;
        let _ = PdhAddEnglishCounterW(
            query,
            w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec"),
            0,
            &mut counter_disk_w,
        );

        // 建立基线（首次没有可计算的速率）
        PdhCollectQueryData(query);

        // 网络吞吐使用 GetIfTable 差分计算（替代不稳定的 PDH 通配符计数器）
        let mut prev_net: Option<(u64, u64, Instant)> = None;

        loop {
            sleep(Duration::from_secs(1));

            // CPU
            let cpu = {
                if PdhCollectQueryData(query) == 0 {
                    let mut value = PDH_FMT_COUNTERVALUE::default();
                    if PdhGetFormattedCounterValue(counter_cpu, PDH_FMT_DOUBLE, None, &mut value) == 0 {
                        value.Anonymous.doubleValue
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            };

            // 磁盘读取（只在 counter 有效时读取）
            let disk_read_bps = {
                let mut value = PDH_FMT_COUNTERVALUE::default();
                if counter_disk_r != 0
                    && PdhGetFormattedCounterValue(counter_disk_r, PDH_FMT_DOUBLE, None, &mut value) == 0
                {
                    value.Anonymous.doubleValue.max(0.0)
                } else {
                    0.0
                }
            };

            let disk_write_bps = {
                let mut value = PDH_FMT_COUNTERVALUE::default();
                if counter_disk_w != 0
                    && PdhGetFormattedCounterValue(counter_disk_w, PDH_FMT_DOUBLE, None, &mut value) == 0
                {
                    value.Anonymous.doubleValue.max(0.0)
                } else {
                    0.0
                }
            };

            let net_total_bps = if let Some((tin, tout)) = read_net_bytes() {
                let now = Instant::now();
                let rate = if let Some((pin, pout, ptime)) = prev_net.take() {
                    let elapsed = now.duration_since(ptime).as_secs_f64();
                    if elapsed > 0.0 {
                        let d = tin.saturating_sub(pin) + tout.saturating_sub(pout);
                        d as f64 / elapsed
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                prev_net = Some((tin, tout, now));
                rate
            } else {
                0.0
            };

            let (mem_load, mem_used_gb, mem_total_gb, mem_page_total_gb, mem_page_used_gb) =
                read_memory().unwrap_or((0, 0.0, 0.0, 0.0, 0.0));

            let snapshot = MetricsSnapshot {
                ts: Local::now().format("%H:%M:%S").to_string(),
                cpu,
                mem_load,
                mem_used_gb,
                mem_total_gb,
                mem_page_total_gb,
                mem_page_used_gb,
                disk_read_bps,
                disk_write_bps,
                net_total_bps,
            };

            // 推入历史环形缓冲区
            crate::history::push(snapshot.clone());

            if app.emit_all("metrics", snapshot).is_err() {
                // 窗口已关闭，退出采集循环
                break;
            }
        }
    }
}
