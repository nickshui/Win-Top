//! 常驻采集器（FastTier）：持有长生命周期的 PDH 查询句柄，
//! 每秒采集一次 CPU + 内存，通过 Tauri 事件推送给前端。
//!
//! 与旧实现的区别：
//!   - PDH 句柄只打开一次，循环复用（旧实现每次 System::new_all 重建，开销巨大）
//!   - CPU 用 PDH English 计数器，规避中文系统本地化 + 修复 sysinfo 恒 0 问题
//!   - 推送式（emit）而非前端每秒 invoke 轮询

use std::mem::size_of;
use std::thread::sleep;
use std::time::Duration;

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use windows::core::w;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[derive(Serialize, Clone)]
pub struct MetricsSnapshot {
    pub ts: String,
    pub cpu: f64,
    pub mem_load: u32,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
}

fn read_memory() -> Option<(u32, f64, f64)> {
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
            ))
        } else {
            None
        }
    }
}

/// 在独立线程中运行。打开一次 PDH 查询，循环采集并 emit。
pub fn run_metrics_loop(app: AppHandle) {
    unsafe {
        let mut query: isize = 0;
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            eprintln!("[collector] PdhOpenQueryW 失败");
            return;
        }

        let mut counter: isize = 0;
        if PdhAddEnglishCounterW(
            query,
            w!("\\Processor(_Total)\\% Processor Time"),
            0,
            &mut counter,
        ) != 0
        {
            eprintln!("[collector] PdhAddEnglishCounterW 失败");
            return;
        }

        // 建立基线（首次没有可计算的速率）
        PdhCollectQueryData(query);

        loop {
            sleep(Duration::from_secs(1));

            let cpu = {
                if PdhCollectQueryData(query) == 0 {
                    let mut value = PDH_FMT_COUNTERVALUE::default();
                    if PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut value) == 0 {
                        value.Anonymous.doubleValue
                    } else {
                        0.0
                    }
                } else {
                    0.0
                }
            };

            let (mem_load, mem_used_gb, mem_total_gb) = read_memory().unwrap_or((0, 0.0, 0.0));

            let snapshot = MetricsSnapshot {
                ts: Local::now().format("%H:%M:%S").to_string(),
                cpu,
                mem_load,
                mem_used_gb,
                mem_total_gb,
            };

            if app.emit_all("metrics", snapshot).is_err() {
                // 窗口已关闭，退出采集循环
                break;
            }
        }
    }
}
