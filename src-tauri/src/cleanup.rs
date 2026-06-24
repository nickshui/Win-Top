//! 垃圾清理：扫描各分类体积、按选择清理。逐文件容错，只在固定根目录内操作，不跟符号链接。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    SHEmptyRecycleBinW, SHQueryRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI,
    SHERB_NOSOUND, SHQUERYRBINFO,
};

#[derive(Serialize)]
pub struct CleanupCategory {
    pub id: String,
    pub label: String,
    pub bytes: u64,
    pub files: u64,
    pub needs_admin: bool,
    pub available: bool,
}

#[derive(Serialize)]
pub struct CleanupReport {
    pub categories: Vec<CleanupCategory>,
    pub total_bytes: u64,
}

#[derive(Serialize)]
pub struct CleanupItemResult {
    pub id: String,
    pub freed_bytes: u64,
    pub skipped: u64,
}

#[derive(Serialize)]
pub struct CleanupResult {
    pub freed_bytes: u64,
    pub items: Vec<CleanupItemResult>,
}

/// 递归统计目录体积与文件数；不存在/不可读返回 (0,0)；不跟符号链接/junction。
fn dir_stats(path: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let md = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let (b, f) = dir_stats(&p);
            bytes += b;
            files += f;
        } else {
            bytes += md.len();
            files += 1;
        }
    }
    (bytes, files)
}

/// 清空目录内容（保留目录本身），逐项容错。符号链接/junction 只删链接本身、绝不递归其目标。
fn clean_dir_contents(path: &Path) -> (u64, u64) {
    let mut freed = 0u64;
    let mut skipped = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let md = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            // 链接/junction：删链接本身（不碰目标）。junction 按目录删，文件链接按文件删。
            if fs::remove_dir(&p).is_ok() || fs::remove_file(&p).is_ok() {
                // 链接本身几乎不占空间，不计入 freed
            } else {
                skipped += 1;
            }
            continue;
        }
        if ft.is_dir() {
            let (b, _) = dir_stats(&p);
            if fs::remove_dir_all(&p).is_ok() {
                freed += b;
            } else {
                skipped += 1;
            }
        } else {
            let sz = md.len();
            if fs::remove_file(&p).is_ok() {
                freed += sz;
            } else {
                skipped += 1;
            }
        }
    }
    (freed, skipped)
}

/// 统计目录中名字以任一前缀开头的文件体积与数量（不递归）。
fn glob_stats(path: &Path, prefixes: &[&str]) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if prefixes.iter().any(|p| n.starts_with(p)) {
                if let Ok(md) = e.metadata() {
                    if md.is_file() {
                        bytes += md.len();
                        files += 1;
                    }
                }
            }
        }
    }
    (bytes, files)
}

/// 删除目录中名字以任一前缀开头的文件，逐项容错。
fn glob_clean(path: &Path, prefixes: &[&str]) -> (u64, u64) {
    let mut freed = 0u64;
    let mut skipped = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if prefixes.iter().any(|p| n.starts_with(p)) {
                let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(e.path()).is_ok() {
                    freed += sz;
                } else {
                    skipped += 1;
                }
            }
        }
    }
    (freed, skipped)
}

enum Kind {
    Dirs,
    Glob(&'static [&'static str]),
    RecycleBin,
}

struct CatDef {
    id: &'static str,
    label: &'static str,
    needs_admin: bool,
    kind: Kind,
}

fn defs() -> Vec<CatDef> {
    vec![
        CatDef { id: "user_temp", label: "用户临时文件", needs_admin: false, kind: Kind::Dirs },
        CatDef { id: "system_temp", label: "系统临时文件", needs_admin: true, kind: Kind::Dirs },
        CatDef { id: "recycle_bin", label: "回收站", needs_admin: false, kind: Kind::RecycleBin },
        CatDef { id: "thumbnails", label: "缩略图缓存", needs_admin: false, kind: Kind::Glob(&["thumbcache_", "iconcache_"]) },
        CatDef { id: "windows_update", label: "Windows Update 缓存", needs_admin: true, kind: Kind::Dirs },
        CatDef { id: "prefetch", label: "Prefetch", needs_admin: true, kind: Kind::Dirs },
        CatDef { id: "edge_cache", label: "Edge 缓存", needs_admin: false, kind: Kind::Dirs },
        CatDef { id: "chrome_cache", label: "Chrome 缓存", needs_admin: false, kind: Kind::Dirs },
    ]
}

fn local_appdata() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

fn windir() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
}

fn browser_cache_dirs(sub: &str) -> Vec<PathBuf> {
    let la = match local_appdata() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let user_data = la.join(sub);
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&user_data) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == "Default" || n.starts_with("Profile") {
                let c = e.path().join("Cache");
                if c.exists() {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn dirs_for(id: &str) -> Vec<PathBuf> {
    match id {
        "user_temp" => {
            let mut v = Vec::new();
            if let Some(t) = std::env::var_os("TEMP") {
                v.push(PathBuf::from(t));
            }
            if let Some(la) = local_appdata() {
                let p = la.join("Temp");
                if !v.contains(&p) {
                    v.push(p);
                }
            }
            v
        }
        "system_temp" => vec![windir().join("Temp")],
        "thumbnails" => local_appdata()
            .map(|la| vec![la.join("Microsoft\\Windows\\Explorer")])
            .unwrap_or_default(),
        "windows_update" => vec![windir().join("SoftwareDistribution\\Download")],
        "prefetch" => vec![windir().join("Prefetch")],
        "edge_cache" => browser_cache_dirs("Microsoft\\Edge\\User Data"),
        "chrome_cache" => browser_cache_dirs("Google\\Chrome\\User Data"),
        _ => Vec::new(),
    }
}

fn recycle_query() -> (u64, u64) {
    unsafe {
        let mut info = SHQUERYRBINFO {
            cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
            ..Default::default()
        };
        if SHQueryRecycleBinW(PCWSTR::null(), &mut info).is_ok() {
            (info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64)
        } else {
            (0, 0)
        }
    }
}

fn recycle_empty() -> (u64, u64) {
    let (bytes, _) = recycle_query();
    unsafe {
        let _ = SHEmptyRecycleBinW(
            HWND::default(),
            PCWSTR::null(),
            SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
        );
    }
    (bytes, 0)
}

pub fn scan_junk() -> CleanupReport {
    let mut cats = Vec::new();
    let mut total = 0u64;
    for d in defs() {
        let (bytes, files, available) = match &d.kind {
            Kind::RecycleBin => {
                let (b, n) = recycle_query();
                (b, n, true)
            }
            Kind::Glob(pref) => {
                let dirs = dirs_for(d.id);
                let avail = dirs.iter().any(|p| p.exists());
                let mut b = 0u64;
                let mut f = 0u64;
                for dir in &dirs {
                    let (bb, ff) = glob_stats(dir, pref);
                    b += bb;
                    f += ff;
                }
                (b, f, avail)
            }
            Kind::Dirs => {
                let dirs = dirs_for(d.id);
                let avail = dirs.iter().any(|p| p.exists());
                let mut b = 0u64;
                let mut f = 0u64;
                for dir in &dirs {
                    let (bb, ff) = dir_stats(dir);
                    b += bb;
                    f += ff;
                }
                (b, f, avail)
            }
        };
        total += bytes;
        cats.push(CleanupCategory {
            id: d.id.into(),
            label: d.label.into(),
            bytes,
            files,
            needs_admin: d.needs_admin,
            available,
        });
    }
    CleanupReport { categories: cats, total_bytes: total }
}

pub fn clean_junk(ids: Vec<String>) -> CleanupResult {
    let mut items = Vec::new();
    let mut total_freed = 0u64;
    for d in defs() {
        if !ids.iter().any(|x| x == d.id) {
            continue;
        }
        let (freed, skipped) = match &d.kind {
            Kind::RecycleBin => recycle_empty(),
            Kind::Glob(pref) => {
                let mut fr = 0u64;
                let mut sk = 0u64;
                for dir in dirs_for(d.id) {
                    let (a, b) = glob_clean(&dir, pref);
                    fr += a;
                    sk += b;
                }
                (fr, sk)
            }
            Kind::Dirs => {
                let mut fr = 0u64;
                let mut sk = 0u64;
                for dir in dirs_for(d.id) {
                    let (a, b) = clean_dir_contents(&dir);
                    fr += a;
                    sk += b;
                }
                (fr, sk)
            }
        };
        total_freed += freed;
        items.push(CleanupItemResult {
            id: d.id.into(),
            freed_bytes: freed,
            skipped,
        });
    }
    CleanupResult { freed_bytes: total_freed, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("wintop_cleanup_test_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn dir_stats_counts_nested() {
        let root = unique_tmp("stats");
        fs::write(root.join("a.txt"), b"hello").unwrap(); // 5
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("b.bin"), vec![0u8; 100]).unwrap(); // 100
        let (bytes, files) = dir_stats(&root);
        assert_eq!(files, 2);
        assert_eq!(bytes, 105);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_removes_contents_keeps_root() {
        let root = unique_tmp("clean");
        fs::write(root.join("x.txt"), vec![0u8; 10]).unwrap();
        let sub = root.join("d");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("y"), vec![0u8; 20]).unwrap();
        let (freed, skipped) = clean_dir_contents(&root);
        assert_eq!(freed, 30);
        assert_eq!(skipped, 0);
        assert!(root.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn glob_targets_prefix_only() {
        let root = unique_tmp("glob");
        fs::write(root.join("thumbcache_1.db"), vec![0u8; 7]).unwrap();
        fs::write(root.join("keep.txt"), vec![0u8; 99]).unwrap();
        let (b, f) = glob_stats(&root, &["thumbcache_"]);
        assert_eq!(f, 1);
        assert_eq!(b, 7);
        let (freed, _sk) = glob_clean(&root, &["thumbcache_"]);
        assert_eq!(freed, 7);
        assert!(root.join("keep.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
