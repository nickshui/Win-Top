//! 磁盘空间分析器：扫描指定目录，产出「全部目录的递归大小 + 全卷最大文件」。
//! 在 spawn_blocking 中运行以避免阻塞 UI；支持进度回调与取消。
//!
//! 与 MFT 扫描保持同一份报告语义（DirSummary 含递归大小与修改时间），
//! 供前端构建 TreeSize 式行内展开树。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;

/// 全局扫描取消标志。空间分析同一时刻只会有一个扫描在跑（整卷或目录），
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
    pub total_size: u64, // 递归大小（含全部子目录）
    pub file_count: u64,
    #[serde(default)]
    pub modified_secs: u64, // 目录自身最后修改时间
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
    /// 扫描根的总大小（含根目录下的直接文件）。
    #[serde(default)]
    pub root_size: u64,
}

/// 按需列出某个目录的直接文件（用于行内展开时展示文件行）。
#[derive(Serialize)]
pub struct DirFileEntry {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_secs: u64,
}

// ─── 定容最小堆：只保留最大的 top_n 个文件 ─────────────────────────────

struct TopFile {
    size: u64,
    path: String,
    modified_secs: u64,
}

impl Eq for TopFile {}
impl PartialEq for TopFile {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
    }
}
impl PartialOrd for TopFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TopFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.size.cmp(&self.size) // 最小堆
    }
}

#[derive(Default)]
struct DirAgg {
    direct_size: u64,
    direct_count: u64,
    modified_secs: u64,
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const MAX_DEPTH: usize = 64;

/// 无进度回调版本的目录扫描（保留给测试/内部调用）。
#[allow(dead_code)]
pub fn scan_directory(dir_path: String, top_n: usize) -> UsageReport {
    scan_directory_with_progress(dir_path, top_n, &mut |_, _| {})
}

/// 扫描 dir_path：收集所有目录的递归大小与修改时间，以及最大的 top_n 个文件。
/// `progress(scanned, total)` 会被周期性调用（walk 无法预知总数，total 为 None）。
pub fn scan_directory_with_progress(
    dir_path: String,
    top_n: usize,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> UsageReport {
    let started = Instant::now();
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
            root_size: 0,
        };
    }

    let mut dirs: HashMap<PathBuf, DirAgg> = HashMap::new();
    dirs.insert(root.clone(), DirAgg::default());
    let mut heap: BinaryHeap<TopFile> = BinaryHeap::with_capacity(top_n + 1);
    let mut scanned = 0u64;
    let mut errors = 0u64;
    let mut cancelled = false;
    let mut last_emit = Instant::now();

    walk(
        &root, &root, 0, &mut dirs, &mut heap, top_n, &mut scanned, &mut errors, &mut cancelled,
        &mut last_emit, progress,
    );

    // 自底向上聚合递归大小 / 文件数（按路径深度倒序处理）
    let mut paths: Vec<PathBuf> = dirs.keys().cloned().collect();
    paths.sort_by_key(|p| Reverse(p.components().count()));
    let mut rec_size: HashMap<PathBuf, u64> =
        dirs.iter().map(|(p, a)| (p.clone(), a.direct_size)).collect();
    let mut rec_count: HashMap<PathBuf, u64> =
        dirs.iter().map(|(p, a)| (p.clone(), a.direct_count)).collect();
    for p in &paths {
        if p == &root {
            continue;
        }
        if let Some(parent) = p.parent() {
            let s = *rec_size.get(p).unwrap_or(&0);
            let c = *rec_count.get(p).unwrap_or(&0);
            *rec_size.entry(parent.to_path_buf()).or_insert(0) += s;
            *rec_count.entry(parent.to_path_buf()).or_insert(0) += c;
        }
    }
    let root_size = *rec_size.get(&root).unwrap_or(&0);

    let mut out_dirs: Vec<DirSummary> = dirs
        .iter()
        .filter(|(p, _)| *p != &root)
        .filter_map(|(p, agg)| {
            let rel = p.strip_prefix(&root).ok()?.to_string_lossy().to_string();
            Some(DirSummary {
                path: rel,
                total_size: *rec_size.get(p).unwrap_or(&0),
                file_count: *rec_count.get(p).unwrap_or(&0),
                modified_secs: agg.modified_secs,
            })
        })
        .collect();
    out_dirs.sort_by(|a, b| b.total_size.cmp(&a.total_size));

    let mut large_files: Vec<LargeFile> = Vec::with_capacity(heap.len());
    while let Some(hf) = heap.pop() {
        large_files.push(LargeFile {
            path: hf.path,
            size: hf.size,
            modified_secs: hf.modified_secs,
        });
    }
    large_files.reverse();

    progress(scanned, None);

    UsageReport {
        large_files,
        dirs: out_dirs,
        scanned,
        errors,
        source: "walk".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled,
        root_size,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    root: &Path,
    current: &Path,
    depth: usize,
    dirs: &mut HashMap<PathBuf, DirAgg>,
    heap: &mut BinaryHeap<TopFile>,
    top_n: usize,
    scanned: &mut u64,
    errors: &mut u64,
    cancelled: &mut bool,
    last_emit: &mut Instant,
    progress: &mut dyn FnMut(u64, Option<u64>),
) {
    if *cancelled || depth > MAX_DEPTH {
        return;
    }
    let rd = match std::fs::read_dir(current) {
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
        *scanned += 1;
        if last_emit.elapsed() >= Duration::from_millis(200) {
            progress(*scanned, None);
            *last_emit = Instant::now();
        }
        let p = entry.path();
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                *errors += 1;
                continue;
            }
        };
        // 跳过符号链接 / 目录联接等重解析点，避免循环与重复计数
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            continue;
        }
        let modified = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if md.is_dir() {
            dirs.entry(p.clone()).or_insert(DirAgg {
                direct_size: 0,
                direct_count: 0,
                modified_secs: modified,
            });
            walk(
                root, &p, depth + 1, dirs, heap, top_n, scanned, errors, cancelled, last_emit,
                progress,
            );
            if *cancelled {
                return;
            }
        } else {
            let sz = md.len();
            if let Some(parent) = p.parent() {
                if let Some(agg) = dirs.get_mut(parent) {
                    agg.direct_size += sz;
                    agg.direct_count += 1;
                }
            }
            if sz > 0 {
                heap.push(TopFile {
                    size: sz,
                    path: p.to_string_lossy().to_string(),
                    modified_secs: modified,
                });
                if heap.len() > top_n {
                    heap.pop();
                }
            }
        }
    }
    let _ = root;
}

/// 列出某目录的直接文件（按大小降序，最多 limit 个），用于展开节点时展示文件行。
pub fn list_dir_files(dir_path: &str, limit: usize) -> Vec<DirFileEntry> {
    let root = PathBuf::from(dir_path);
    let mut out: Vec<DirFileEntry> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for entry in rd.flatten() {
            let p = entry.path();
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.is_dir() {
                continue;
            }
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
            let modified = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(DirFileEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: p.to_string_lossy().to_string(),
                size: md.len(),
                modified_secs: modified,
            });
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_report_has_recursive_sizes() {
        // 构造临时目录树：root/a/file1(100) / root/a/b/file2(50) / root/file3(10)
        let root = std::env::temp_dir().join(format!("wintop-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("a").join("file1"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("a").join("b").join("file2"), vec![0u8; 50]).unwrap();
        std::fs::write(root.join("file3"), vec![0u8; 10]).unwrap();

        let report = scan_directory(root.to_string_lossy().to_string(), 5);
        assert_eq!(report.source, "walk");
        assert_eq!(report.root_size, 160);
        let dir_a = report.dirs.iter().find(|d| d.path == "a").expect("dir a");
        assert_eq!(dir_a.total_size, 150);
        assert_eq!(dir_a.file_count, 2);
        let dir_b = report.dirs.iter().find(|d| d.path == "a\\b").expect("dir b");
        assert_eq!(dir_b.total_size, 50);
        assert_eq!(report.large_files[0].size, 100);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_dir_files_returns_files_only() {
        let root = std::env::temp_dir().join(format!("wintop-list-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.bin"), vec![0u8; 5]).unwrap();
        std::fs::write(root.join("b.bin"), vec![0u8; 9]).unwrap();
        let files = list_dir_files(&root.to_string_lossy(), 10);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].size, 9); // 按大小降序
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod real_tests {
    #[test]
    #[ignore = "真机冒烟：扫描 WINTOP_SCAN_TEST_DIR 指定的目录"]
    fn scan_real_dir_smoke() {
        let dir = match std::env::var("WINTOP_SCAN_TEST_DIR") {
            Ok(d) => d,
            Err(_) => return,
        };
        let r = super::scan_directory(dir.clone(), 10);
        println!(
            "dir={:?} exists={} scanned={} errors={} root_size={} dirs={} files={} elapsed={}ms src={} cancelled={}",
            dir,
            std::path::Path::new(&dir).exists(),
            r.scanned, r.errors, r.root_size, r.dirs.len(), r.large_files.len(),
            r.elapsed_ms, r.source, r.cancelled
        );
        for f in &r.large_files { println!("file {} {}", f.size, f.path); }
    }
}
