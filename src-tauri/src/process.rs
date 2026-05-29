//! 进程模块：NtQuerySystemInformation 一次性枚举全量进程，
//! 通过两次采样的内核+用户时间差计算准确 CPU%。
//! 进程操作（结束/优先级）用 WinAPI 直调，不再 spawn powershell。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use serde::Serialize;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::{CloseHandle, BOOL};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{
    OpenProcess, SetPriorityClass, TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS,
    BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_CREATION_FLAGS, PROCESS_SET_INFORMATION, PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS,
};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

#[derive(Serialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub name: String,
    pub cpu: f64,
    pub mem_mb: f64,
    pub threads: u32,
}

#[derive(Serialize)]
pub struct ActionResult {
    pub success: bool,
    pub message: String,
}

struct CpuTracker {
    prev: HashMap<u32, i64>,
    last: Option<Instant>,
}

static TRACKER: LazyLock<Mutex<CpuTracker>> = LazyLock::new(|| {
    Mutex::new(CpuTracker {
        prev: HashMap::new(),
        last: None,
    })
});

static NCPU: LazyLock<f64> = LazyLock::new(|| unsafe {
    let mut si = SYSTEM_INFO::default();
    GetSystemInfo(&mut si);
    (si.dwNumberOfProcessors.max(1)) as f64
});

/// 原始进程项（pid, 名称, 内核+用户时间(100ns), 工作集字节, 线程数）
struct RawProc {
    pid: u32,
    name: String,
    total_time: i64,
    ws: u64,
    threads: u32,
}

unsafe fn query_processes() -> Result<Vec<RawProc>, String> {
    let mut len: u32 = 512 * 1024;
    let mut buf: Vec<u8> = vec![0u8; len as usize];

    loop {
        let mut ret: u32 = 0;
        let status = NtQuerySystemInformation(
            SystemProcessInformation,
            buf.as_mut_ptr() as *mut c_void,
            len,
            &mut ret,
        );
        if status.0 == STATUS_INFO_LENGTH_MISMATCH {
            len = ret.max(len * 2);
            buf = vec![0u8; len as usize];
            continue;
        }
        if status.is_ok() {
            break;
        }
        return Err(format!("NtQuerySystemInformation 失败: 0x{:08X}", status.0));
    }

    let mut out = Vec::new();
    let mut offset = 0usize;
    loop {
        let p = buf.as_ptr().add(offset) as *const SYSTEM_PROCESS_INFORMATION;
        let info = &*p;

        let pid = info.UniqueProcessId.0 as u32;
        let name = if info.ImageName.Buffer.0.is_null() {
            if pid == 0 {
                "System Idle Process".to_string()
            } else {
                String::new()
            }
        } else {
            let chars = (info.ImageName.Length / 2) as usize;
            let slice = std::slice::from_raw_parts(info.ImageName.Buffer.0, chars);
            String::from_utf16_lossy(slice)
        };

        // 0.58 的结构体把时间字段藏在 Reserved1[48] 内：
        //   [0..8] WorkingSetPrivateSize, [8..12] HardFaultCount,
        //   [12..16] ThreadsHighWatermark, [16..24] CycleTime,
        //   [24..32] CreateTime, [32..40] UserTime, [40..48] KernelTime
        let user_time = i64::from_le_bytes(info.Reserved1[32..40].try_into().unwrap());
        let kernel_time = i64::from_le_bytes(info.Reserved1[40..48].try_into().unwrap());

        out.push(RawProc {
            pid,
            name,
            total_time: user_time + kernel_time,
            ws: info.WorkingSetSize as u64,
            threads: info.NumberOfThreads,
        });

        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
    Ok(out)
}

/// 供其它模块（如网络端口）复用的 PID -> 进程名映射。
pub fn pid_name_map() -> HashMap<u32, String> {
    match unsafe { query_processes() } {
        Ok(list) => list
            .into_iter()
            .map(|p| {
                let name = if p.name.is_empty() {
                    format!("PID {}", p.pid)
                } else {
                    p.name
                };
                (p.pid, name)
            })
            .collect(),
        Err(_) => HashMap::new(),
    }
}

pub fn list_processes() -> Result<Vec<ProcessRow>, String> {
    let snapshot = unsafe { query_processes()? };
    let now = Instant::now();
    let ncpu = *NCPU;

    let mut tracker = TRACKER.lock().map_err(|_| "tracker lock 失败".to_string())?;
    let elapsed = tracker
        .last
        .map(|t| now.duration_since(t).as_secs_f64())
        .unwrap_or(0.0);

    let mut rows = Vec::with_capacity(snapshot.len());
    let mut new_prev = HashMap::with_capacity(snapshot.len());

    for rp in &snapshot {
        let cpu = if elapsed > 0.0 {
            if let Some(&prev_total) = tracker.prev.get(&rp.pid) {
                let delta_secs = (rp.total_time - prev_total) as f64 * 1e-7;
                (delta_secs / (elapsed * ncpu) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            }
        } else {
            0.0
        };
        new_prev.insert(rp.pid, rp.total_time);
        rows.push(ProcessRow {
            pid: rp.pid,
            name: if rp.name.is_empty() {
                format!("PID {}", rp.pid)
            } else {
                rp.name.clone()
            },
            cpu,
            mem_mb: rp.ws as f64 / 1024.0 / 1024.0,
            threads: rp.threads,
        });
    }

    tracker.prev = new_prev;
    tracker.last = Some(now);

    rows.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    Ok(rows)
}

pub fn terminate(pid: u32) -> ActionResult {
    unsafe {
        match OpenProcess(PROCESS_TERMINATE, BOOL(0), pid) {
            Ok(handle) => {
                let result = TerminateProcess(handle, 1);
                let _ = CloseHandle(handle);
                match result {
                    Ok(_) => ActionResult {
                        success: true,
                        message: format!("进程已结束（PID {}）。", pid),
                    },
                    Err(e) => ActionResult {
                        success: false,
                        message: format!("结束进程失败：{}", e.message()),
                    },
                }
            }
            Err(e) => ActionResult {
                success: false,
                message: format!("无法打开进程（可能需要管理员权限）：{}", e.message()),
            },
        }
    }
}

fn map_priority(level: &str) -> Option<PROCESS_CREATION_FLAGS> {
    match level {
        "低" | "Idle" => Some(IDLE_PRIORITY_CLASS),
        "低于普通" | "BelowNormal" => Some(BELOW_NORMAL_PRIORITY_CLASS),
        "普通" | "Normal" => Some(NORMAL_PRIORITY_CLASS),
        "高于普通" | "AboveNormal" => Some(ABOVE_NORMAL_PRIORITY_CLASS),
        "高" | "High" => Some(HIGH_PRIORITY_CLASS),
        "实时" | "RealTime" => Some(REALTIME_PRIORITY_CLASS),
        _ => None,
    }
}

pub fn set_priority(pid: u32, level: &str) -> ActionResult {
    let class = match map_priority(level) {
        Some(c) => c,
        None => {
            return ActionResult {
                success: false,
                message: format!("无效的优先级：{}", level),
            }
        }
    };
    unsafe {
        match OpenProcess(PROCESS_SET_INFORMATION, BOOL(0), pid) {
            Ok(handle) => {
                let result = SetPriorityClass(handle, class);
                let _ = CloseHandle(handle);
                match result {
                    Ok(_) => ActionResult {
                        success: true,
                        message: format!("优先级已设为「{}」（PID {}）。", level, pid),
                    },
                    Err(e) => {
                        let hint = if level == "实时" {
                            "（实时优先级通常需要管理员权限）"
                        } else {
                            ""
                        };
                        ActionResult {
                            success: false,
                            message: format!("设置优先级失败{}：{}", hint, e.message()),
                        }
                    }
                }
            }
            Err(e) => ActionResult {
                success: false,
                message: format!("无法打开进程（可能需要管理员权限）：{}", e.message()),
            },
        }
    }
}
