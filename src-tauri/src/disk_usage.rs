//! 磁盘空间分析器：扫描指定目录找出大文件，按类型/目录统计。
//! 在 spawn_blocking 中运行以避免阻塞 UI。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use serde::Serialize;

/// 全局扫描取消标志。空间分析同一时刻只会有一个扫描在跑（整卷或下钻），
/// 用进程级标志就足够满足"用户点取消→尽快停止"的需求，无需按扫描实例区分。
static SCAN_CANCELLED: AtomicBool = AtomicBool::new(false);

/// 每次发起新扫描前调用，清掉上一次可能残留的取消标志。
pub fn reset_cancel() {
    SCAN_CANCELLED.store(false, Ordering::Relaxed);
}

/// 前端"取消扫描"调用：请求正在运行的扫描尽快停止。
pub fn request_cancel() {
    SCAN_CANCELLED.store(true, Ordering::Relaxed);
}

pub fn is_cancelled() -> bool {
    SCAN_CANCELLED.load(Ordering::Relaxed)
}

#[derive(Serialize)]
pub struct LargeFile {
    pub path: String,
    pub size: u64,
    pub modified_secs: u64,
}

#[derive(Serialize)]
pub struct DirSummary {
    pub path: String,
    pub total_size: u64,
    pub file_count: u64,
}

#[derive(Serialize)]
pub struct UsageReport {
    pub large_files: Vec<LargeFile>,
    pub dirs: Vec<DirSummary>,
    pub scanned: u64,
    pub errors: u64,
    #[serde(default)]
    pub source: String, // "mft" | "walk"
    #[serde(default)]
    pub elapsed_ms: u64,
    /// 是否因用户取消而提前结束（此时其余字段为取消前的部分结果）。
    #[serde(default)]
    pub cancelled: bool,
}

/// 扫描 dir_path，收集最大的 top_n 个文件，并统计直接子目录的体积。
/// max_depth 限制递归深度（默认 8，防符号链接循环）。
pub fn scan_directory(dir_path: String, top_n: usize) -> UsageReport {
    let started = std::time::Instant::now();
    let root = PathBuf::from(&dir_path);
    if !root.exists() {
        return UsageReport {
            large_files: vec![],
            dirs: vec![],
            scanned: 0,
            errors: 0,
            source: "walk".into(),
            elapsed_ms: 0,
            cancelled: false,
        };
    }

    let mut files: Vec<LargeFile> = Vec::new();
    let mut dir_map: HashMap<String, (u64, u64)> = HashMap::new();
    let mut scanned = 0u64;
    let mut errors = 0u64;
    let mut cancelled = false;

    collect(
        &root, &root, 0, 8, &mut files, &mut dir_map, &mut scanned, &mut errors, &mut cancelled,
    );

    // Sort files by size descending, take top_n
    files.sort_by(|a, b| b.size.cmp(&a.size));
    files.truncate(top_n);

    // Convert dir_map to sorted vec
    let mut dirs: Vec<DirSummary> = dir_map
        .into_iter()
        .map(|(path, (total_size, file_count))| DirSummary {
            path,
            total_size,
            file_count,
        })
        .collect();
    dirs.sort_by(|a, b| b.total_size.cmp(&a.total_size));

    UsageReport {
        large_files: files,
        dirs,
        scanned,
        errors,
        source: "walk".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled,
    }
}

fn collect(
    root: &Path,
    current: &Path,
    depth: usize,
    max_depth: usize,
    files: &mut Vec<LargeFile>,
    dirs: &mut HashMap<String, (u64, u64)>,
    scanned: &mut u64,
    errors: &mut u64,
    cancelled: &mut bool,
) {
    if *cancelled {
        return;
    }
    if depth > max_depth {
        return;
    }
    let rd = match fs::read_dir(current) {
        Ok(r) => r,
        Err(_) => {
            *errors += 1;
            return;
        }
    };
    for entry in rd.flatten() {
        if is_cancelled() {
            *cancelled = true;
            return;
        }
        let p = entry.path();
        *scanned += 1;
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                *errors += 1;
                continue;
            }
        };
        if md.is_symlink() {
            continue;
        }
        if md.is_dir() {
            collect(root, &p, depth + 1, max_depth, files, dirs, scanned, errors, cancelled);
            if *cancelled {
                return;
            }
        } else {
            let sz = md.len();
            let modified = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            files.push(LargeFile {
                path: p.to_string_lossy().to_string(),
                size: sz,
                modified_secs: modified,
            });
            // Accumulate into parent dir (relative to root)
            if let Some(parent) = p.parent() {
                if let Ok(rel) = parent.strip_prefix(root) {
                    let key = rel.to_string_lossy().to_string();
                    let e = dirs.entry(key).or_insert((0, 0));
                    e.0 += sz;
                    e.1 += 1;
                }
            }
        }
    }
}
