//! 内存加速：裁剪进程工作集释放物理内存；给出"建议关闭的后台进程"候选（不自动结束）。

use std::collections::HashSet;
use std::mem::size_of;

use serde::Serialize;

use windows::Win32::Foundation::{BOOL, CloseHandle};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA};

#[derive(Serialize)]
pub struct BoostResult {
    pub freed_mb: f64,
    pub trimmed_count: u32,
    pub before_avail_mb: f64,
    pub after_avail_mb: f64,
}

#[derive(Serialize, Clone)]
pub struct BgProc {
    pub pid: u32,
    pub name: String,
    pub mem_mb: f64,
}

fn avail_mb() -> f64 {
    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if GlobalMemoryStatusEx(&mut ms).is_ok() {
            ms.ullAvailPhys as f64 / 1024.0 / 1024.0
        } else {
            0.0
        }
    }
}

/// 关键进程 denylist（小写）。这些不进"建议关闭"候选。
fn deny_set() -> HashSet<&'static str> {
    [
        "system", "system idle process", "registry", "smss.exe", "csrss.exe",
        "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe",
        "fontdrvhost.exe", "dwm.exe", "explorer.exe", "sihost.exe", "ctfmon.exe",
        "conhost.exe", "runtimebroker.exe", "win-top.exe",
    ]
    .into_iter()
    .collect()
}

/// 纯函数：从 (pid,name,mem_mb) 列表挑"建议关闭的后台进程"。
/// 排除 denylist（大小写不敏感）与 pid 0/4，仅留 mem_mb>=threshold，按内存降序取前 n。
fn pick_background(
    procs: &[(u32, String, f64)],
    deny: &HashSet<&str>,
    threshold_mb: f64,
    n: usize,
) -> Vec<BgProc> {
    let mut v: Vec<BgProc> = procs
        .iter()
        .filter(|(pid, name, mem)| {
            *pid != 0 && *pid != 4 && *mem >= threshold_mb
                && !deny.contains(name.to_lowercase().as_str())
        })
        .map(|(pid, name, mem)| BgProc { pid: *pid, name: name.clone(), mem_mb: *mem })
        .collect();
    v.sort_by(|a, b| b.mem_mb.partial_cmp(&a.mem_mb).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(n);
    v
}

pub fn memory_boost() -> BoostResult {
    let before = avail_mb();
    let mut trimmed = 0u32;
    if let Ok(list) = crate::process::list_processes() {
        for p in &list {
            unsafe {
                if let Ok(h) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_INFORMATION, BOOL(0), p.pid) {
                    if EmptyWorkingSet(h).is_ok() {
                        trimmed += 1;
                    }
                    let _ = CloseHandle(h);
                }
            }
        }
    }
    let after = avail_mb();
    BoostResult {
        freed_mb: (after - before).max(0.0),
        trimmed_count: trimmed,
        before_avail_mb: before,
        after_avail_mb: after,
    }
}

pub fn suggest_background() -> Vec<BgProc> {
    let procs: Vec<(u32, String, f64)> = match crate::process::list_processes() {
        Ok(list) => list.into_iter().map(|p| (p.pid, p.name, p.mem_mb)).collect(),
        Err(_) => Vec::new(),
    };
    pick_background(&procs, &deny_set(), 50.0, 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_filters_sorts_truncates() {
        let deny = deny_set();
        let procs = vec![
            (0u32, "System Idle Process".to_string(), 9999.0), // pid 0 排除
            (10, "svchost.exe".to_string(), 500.0),            // deny 排除
            (20, "chrome.exe".to_string(), 300.0),
            (30, "game.exe".to_string(), 900.0),
            (40, "tiny.exe".to_string(), 10.0),                // 低于阈值排除
            (50, "editor.exe".to_string(), 120.0),
        ];
        let got = pick_background(&procs, &deny, 50.0, 2);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "game.exe"); // 900 最大
        assert_eq!(got[1].name, "chrome.exe"); // 300 次之
    }
}
