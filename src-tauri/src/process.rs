//! 进程模块：NtQuerySystemInformation 一次性枚举全量进程，
//! 通过两次采样的内核+用户时间差计算准确 CPU%。
//! 进程操作（结束/优先级）用 WinAPI 直调，不再 spawn powershell。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::{CloseHandle, BOOL};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessHandleCount, OpenProcess, SetPriorityClass, TerminateProcess,
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_CREATION_FLAGS,
    PROCESS_QUERY_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS,
};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;

use wmi::{COMLibrary, WMIConnection};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

#[derive(Serialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
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

/// 原始进程项（pid, 父pid, 名称, 内核+用户时间(100ns), 工作集字节, 线程数）
struct RawProc {
    pid: u32,
    parent_pid: u32,
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
        // InheritedFromUniqueProcessId 紧跟在 UniqueProcessId 之后（HANDLE=8 字节）
        let parent_pid = {
            let upid: *const windows::Win32::Foundation::HANDLE = &info.UniqueProcessId;
            unsafe { (*upid.add(1)).0 as u32 }
        };
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
            parent_pid,
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

/// 返回当前所有进程的 PID 列表（轻量，不含 CPU 跟踪开销）。
pub fn list_pids() -> Vec<u32> {
    unsafe {
        query_processes()
            .map(|v| v.into_iter().map(|p| p.pid).collect())
            .unwrap_or_default()
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
            parent_pid: rp.parent_pid,
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

// ===== 进程详情（P1.4） =====

#[derive(Serialize)]
pub struct ProcessDetail {
    pub pid: u32,
    pub name: String,
    pub command_line: String,
    pub full_path: String,
    pub parent_pid: u32,
    pub cpu: f64,
    pub mem_mb: f64,
    pub threads: u32,
    pub handles: u32,
    pub priority: String,
}

unsafe fn get_handle_count(pid: u32) -> u32 {
    let handle = match OpenProcess(PROCESS_QUERY_INFORMATION, BOOL(0), pid) {
        Ok(h) => h,
        Err(_) => return 0,
    };
    let mut count: u32 = 0;
    let ok = GetProcessHandleCount(handle, &mut count);
    let _ = CloseHandle(handle);
    if ok.is_ok() { count } else { 0 }
}

unsafe fn get_priority_label(pid: u32) -> String {
    let handle = match OpenProcess(PROCESS_QUERY_INFORMATION, BOOL(0), pid) {
        Ok(h) => h,
        Err(_) => return "未知".to_string(),
    };
    let pc = GetPriorityClass(handle);
    let _ = CloseHandle(handle);
    if pc == IDLE_PRIORITY_CLASS.0 {
        "低".to_string()
    } else if pc == BELOW_NORMAL_PRIORITY_CLASS.0 {
        "低于普通".to_string()
    } else if pc == NORMAL_PRIORITY_CLASS.0 {
        "普通".to_string()
    } else if pc == ABOVE_NORMAL_PRIORITY_CLASS.0 {
        "高于普通".to_string()
    } else if pc == HIGH_PRIORITY_CLASS.0 {
        "高".to_string()
    } else if pc == REALTIME_PRIORITY_CLASS.0 {
        "实时".to_string()
    } else {
        "未知".to_string()
    }
}

fn wmi_detail(pid: u32) -> (String, String) {
    // WMI 初始化需独立线程（避免与 Tauri STA 套间冲突）
    let handle = std::thread::spawn(move || -> Result<(String, String), String> {
        let com = COMLibrary::new().map_err(|e| e.to_string())?;
        let con = WMIConnection::new(com).map_err(|e| e.to_string())?;

        #[derive(Deserialize)]
        #[serde(rename = "Win32_Process")]
        #[serde(rename_all = "PascalCase")]
        struct Win32Process {
            command_line: Option<String>,
            executable_path: Option<String>,
        }

        let results: Vec<Win32Process> = con
            .raw_query(&format!(
                "SELECT CommandLine, ExecutablePath FROM Win32_Process WHERE ProcessId = {}",
                pid
            ))
            .map_err(|e| e.to_string())?;

        match results.into_iter().next() {
            Some(p) => Ok((
                p.command_line.unwrap_or_default(),
                p.executable_path.unwrap_or_default(),
            )),
            None => Ok((String::new(), String::new())),
        }
    });

    match handle.join() {
        Ok(Ok((cmd, path))) => (cmd, path),
        _ => (String::new(), String::new()),
    }
}

/// 获取单个进程的详细信息（供详情面板使用）。
pub fn get_process_detail(pid: u32) -> Result<ProcessDetail, String> {
    let procs = unsafe { query_processes()? };
    let proc = procs
        .iter()
        .find(|p| p.pid == pid)
        .ok_or_else(|| format!("进程 PID {} 未找到", pid))?;

    // CPU 从 list_processes 获取（含差分计算）
    let rows = list_processes()?;
    let cpu = rows.iter().find(|r| r.pid == pid).map(|r| r.cpu).unwrap_or(0.0);

    let handles = unsafe { get_handle_count(pid) };
    let priority = unsafe { get_priority_label(pid) };
    let (command_line, full_path) = wmi_detail(pid);

    Ok(ProcessDetail {
        pid,
        name: if proc.name.is_empty() {
            format!("PID {}", pid)
        } else {
            proc.name.clone()
        },
        command_line,
        full_path,
        parent_pid: proc.parent_pid,
        cpu,
        mem_mb: proc.ws as f64 / 1024.0 / 1024.0,
        threads: proc.threads,
        handles,
        priority,
    })
}
