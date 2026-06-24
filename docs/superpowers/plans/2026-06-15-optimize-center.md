# 优化中心（清理 / 内存加速 / 启动项）实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把占位的「工具箱」改造成优化中心：一键垃圾清理 + 内存释放 + 启动项启用/禁用。

**Architecture:** 三个独立后端模块（cleanup / memboost / startup），各自纯函数 TDD + Windows API 外壳，按需 `invoke`（无常驻事件）；前端 `Optimize.svelte` 两个子标签呈现。

**Tech Stack:** Rust + Tauri 1.5 + windows-rs 0.58；Svelte 4。

## Global Constraints

- 仅 Windows；所有新模块在 `main.rs` 用 `#[cfg(target_os = "windows")]` 声明。
- 不引新 crate：注册表用 windows-rs `Win32_System_Registry`，回收站用 `Win32_UI_Shell`，内存裁剪用 `Win32_System_ProcessStatus`。
- 破坏性清理逐文件容错（被占用就跳过计数），只在固定根目录内操作，不跟符号链接。
- 关闭后台进程复用现有 `terminate_process` 命令，绝不自动执行。
- 提交信息**不要**任何 AI 署名 / Co-Authored-By / Generated-with 尾注。
- release 构建必须 `cargo build --release --features custom-protocol`；替换 exe 前先 `powershell Stop-Process -Name win-top -Force`。
- 单元测试只碰自建临时目录，**绝不**碰真实系统路径。
- 验证：纯函数 `cargo test`；Windows API 外壳 `cargo check`（提权运行时行为留作用户实测）；前端 `npm run build`。

---

## 文件结构

- Create `src-tauri/src/cleanup.rs` — 垃圾扫描/清理（纯统计/删除函数 + 分类定义 + 回收站）
- Create `src-tauri/src/memboost.rs` — 内存释放 + 后台进程建议
- Create `src-tauri/src/startup.rs` — 启动项枚举 + 启用/禁用
- Modify `src-tauri/Cargo.toml` — 新增 features `Win32_System_ProcessStatus`、`Win32_System_Registry`
- Modify `src-tauri/src/main.rs` — 三个 `mod` + 六个命令 + 注册
- Create `src/lib/views/Optimize.svelte` — 优化中心视图（两个子标签）
- Modify `src/App.svelte` — `toolbox` 路由指向 `Optimize`
- Modify `src/lib/components/Sidebar.svelte` — `toolbox` 标签「工具箱」→「优化加速」

---

## Task 1: 垃圾清理模块 cleanup.rs

**Files:**
- Create: `src-tauri/src/cleanup.rs`
- Modify: `src-tauri/src/main.rs`（`mod cleanup;` + 两个命令 + 注册）

**Interfaces:**
- Produces: `cleanup::scan_junk() -> CleanupReport`、`cleanup::clean_junk(ids: Vec<String>) -> CleanupResult`。结构体 `CleanupReport { categories: Vec<CleanupCategory{id,label,bytes,files,needs_admin,available}>, total_bytes }`、`CleanupResult { freed_bytes, items: Vec<CleanupItemResult{id,freed_bytes,skipped}> }`。

- [ ] **Step 1: 写纯函数测试 + 文件骨架**

创建 `src-tauri/src/cleanup.rs`：

```rust
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

/// 递归统计目录体积与文件数；不存在/不可读返回 (0,0)；不跟符号链接。
fn dir_stats(path: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for entry in rd.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let (b, f) = dir_stats(&entry.path());
            bytes += b;
            files += f;
        } else if let Ok(md) = entry.metadata() {
            bytes += md.len();
            files += 1;
        }
    }
    (bytes, files)
}

/// 清空目录内容（保留目录本身），逐项容错。
fn clean_dir_contents(path: &Path) -> (u64, u64) {
    let mut freed = 0u64;
    let mut skipped = 0u64;
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if ft.is_dir() && !ft.is_symlink() {
            let (b, _) = dir_stats(&p);
            if fs::remove_dir_all(&p).is_ok() {
                freed += b;
            } else {
                skipped += 1;
            }
        } else {
            let sz = entry.metadata().map(|m| m.len()).unwrap_or(0);
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
```

Also add the module declaration so the crate compiles the tests. In `src-tauri/src/main.rs`, after `#[cfg(target_os = "windows")] mod cleanup` does not exist yet — add after the `mod collector;` block near the top (alongside the other `#[cfg(target_os = "windows")] mod X;` lines):

```rust
#[cfg(target_os = "windows")]
mod cleanup;
```

- [ ] **Step 2: 运行纯函数测试确认通过**

Run: `cd src-tauri && cargo test cleanup`
Expected: `test result: ok. 3 passed`。（这些是纯函数，直接通过即可——本任务的 TDD 价值在于先有测试覆盖删除/统计逻辑。）

- [ ] **Step 3: 加分类定义 + 扫描/清理外壳**

在 `cleanup.rs` 的 `#[cfg(test)]` 之前，追加：

```rust
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
```

- [ ] **Step 4: 加命令并注册**

在 `src-tauri/src/main.rs` 的 `get_nettraffic_status` 命令之后加两个命令（清理可能较慢，放 `spawn_blocking`，与 `speed_test` 同模式）：

```rust
#[cfg(target_os = "windows")]
#[tauri::command]
async fn scan_junk() -> cleanup::CleanupReport {
    tauri::async_runtime::spawn_blocking(cleanup::scan_junk)
        .await
        .unwrap_or_else(|_| cleanup::CleanupReport { categories: vec![], total_bytes: 0 })
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn clean_junk(ids: Vec<String>) -> cleanup::CleanupResult {
    tauri::async_runtime::spawn_blocking(move || cleanup::clean_junk(ids))
        .await
        .unwrap_or_else(|_| cleanup::CleanupResult { freed_bytes: 0, items: vec![] })
}
```

在 `tauri::generate_handler![ ... ]` 列表末尾（`get_nettraffic_status` 后）加逗号并追加：

```rust
            get_nettraffic_status,
            scan_junk,
            clean_junk
```

- [ ] **Step 5: 编译 + 测试**

Run: `cd src-tauri && cargo check`
Expected: `Finished`（SH* 若签名不符，按编译错误对照 windows-rs 修正，勿改结构）。

Run: `cd src-tauri && cargo test cleanup`
Expected: `test result: ok. 3 passed`。

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/cleanup.rs src-tauri/src/main.rs
git commit -m "feat(cleanup): 垃圾扫描/清理模块(8 分类 + 回收站)"
```

---

## Task 2: 内存加速模块 memboost.rs

**Files:**
- Create: `src-tauri/src/memboost.rs`
- Modify: `src-tauri/Cargo.toml`（windows features 加 `Win32_System_ProcessStatus`）
- Modify: `src-tauri/src/main.rs`（`mod memboost;` + 两个命令 + 注册）

**Interfaces:**
- Consumes: `crate::process::list_processes() -> Result<Vec<ProcessRow{pid:u32,name:String,mem_mb:f64,..}>, String>`（已存在）。
- Produces: `memboost::memory_boost() -> BoostResult{freed_mb,trimmed_count,before_avail_mb,after_avail_mb}`、`memboost::suggest_background() -> Vec<BgProc{pid,name,mem_mb}>`。

- [ ] **Step 1: 加 Cargo feature**

在 `src-tauri/Cargo.toml` 的 `windows = { version = "0.58", features = [ ... ] }` 列表里加一行：

```toml
  "Win32_System_ProcessStatus",
```

- [ ] **Step 2: 写模块 + 纯函数单测**

创建 `src-tauri/src/memboost.rs`：

```rust
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
                    if EmptyWorkingSet(h).as_bool() {
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
```

注：`EmptyWorkingSet` 在 windows 0.58 返回 `BOOL`，用 `.as_bool()`。若 `cargo check` 报它返回 `Result<()>`，改用 `.is_ok()`。

在 `src-tauri/src/main.rs` 顶部加：

```rust
#[cfg(target_os = "windows")]
mod memboost;
```

- [ ] **Step 3: 加命令并注册**

在 `main.rs` 的 `clean_junk` 命令之后加：

```rust
#[cfg(target_os = "windows")]
#[tauri::command]
async fn memory_boost() -> memboost::BoostResult {
    tauri::async_runtime::spawn_blocking(memboost::memory_boost)
        .await
        .unwrap_or_else(|_| memboost::BoostResult {
            freed_mb: 0.0,
            trimmed_count: 0,
            before_avail_mb: 0.0,
            after_avail_mb: 0.0,
        })
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn suggest_background() -> Vec<memboost::BgProc> {
    memboost::suggest_background()
}
```

在 `generate_handler!` 列表里 `clean_junk` 后加逗号并追加：

```rust
            clean_junk,
            memory_boost,
            suggest_background
```

- [ ] **Step 4: 编译 + 测试**

Run: `cd src-tauri && cargo check`
Expected: `Finished`（若 `OpenProcess` 第二参/EmptyWorkingSet 返回类型不符，按编译错误对照 windows-rs 修正，勿改结构）。

Run: `cd src-tauri && cargo test memboost`
Expected: `test result: ok. 1 passed`。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/memboost.rs src-tauri/Cargo.toml src-tauri/src/main.rs
git commit -m "feat(memboost): 内存释放(EmptyWorkingSet) + 后台进程建议"
```

---

## Task 3: 启动项模块 startup.rs

**Files:**
- Create: `src-tauri/src/startup.rs`
- Modify: `src-tauri/Cargo.toml`（windows features 加 `Win32_System_Registry`）
- Modify: `src-tauri/src/main.rs`（`mod startup;` + 两个命令 + 注册）

**Interfaces:**
- Consumes: `crate::process::ActionResult{success:bool,message:String}`（已存在）。
- Produces: `startup::list_startup() -> Vec<StartupItem{id,name,command,location,enabled}>`、`startup::set_startup_enabled(id:String, enabled:bool) -> ActionResult`。

**说明：** 这是注册表 FFI，签名细节（`REG_VALUE_TYPE`/`REG_OPEN_CREATE_OPTIONS` 等 newtype）以 `cargo check` 为准微调，**逻辑不变**。纯函数 `parse_approved_state`/`encode_approved` 有单测；注册表读写靠 `cargo check` + 用户提权实测。

- [ ] **Step 1: 加 Cargo feature**

在 `src-tauri/Cargo.toml` 的 windows features 列表加：

```toml
  "Win32_System_Registry",
```

- [ ] **Step 2: 写模块 + 纯函数单测**

创建 `src-tauri/src/startup.rs`：

```rust
//! 启动项：枚举 Run 键 + 启动文件夹，交叉 StartupApproved 判启用态；启用/禁用写 StartupApproved（可逆，不删原项）。

use std::path::PathBuf;

use serde::Serialize;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY,
    REG_OPTION_NON_VOLATILE,
};

pub use crate::process::ActionResult;

#[derive(Serialize)]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub location: String, // HKCU-Run | HKLM-Run | User-Folder | Common-Folder
    pub enabled: bool,
}

const RUN_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED_RUN: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
const APPROVED_FOLDER: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder";

/// 纯函数：StartupApproved 二进制值首字节判启用（缺值/0x02/0x06 视为启用，0x03 禁用）。
fn parse_approved_state(bytes: &[u8]) -> bool {
    !matches!(bytes.first(), Some(0x03))
}

/// 纯函数：启用/禁用对应的 12 字节 StartupApproved 值。
fn encode_approved(enabled: bool) -> [u8; 12] {
    let mut b = [0u8; 12];
    b[0] = if enabled { 0x02 } else { 0x03 };
    b
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读某 hive 下 Run 键的所有 (name, command)。
unsafe fn read_run_values(hive: HKEY) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let path = to_wide(RUN_PATH);
    let mut hkey = HKEY::default();
    if RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
        return out;
    }
    let mut index = 0u32;
    loop {
        let mut name_buf = [0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let mut data_buf = [0u8; 8192];
        let mut data_len = data_buf.len() as u32;
        let r = RegEnumValueW(
            hkey,
            index,
            PWSTR(name_buf.as_mut_ptr()),
            &mut name_len,
            None,
            None,
            Some(data_buf.as_mut_ptr()),
            Some(&mut data_len),
        );
        if r == ERROR_NO_MORE_ITEMS || r != ERROR_SUCCESS {
            break;
        }
        let name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
        let wlen = (data_len as usize) / 2;
        let cmd_u16 = std::slice::from_raw_parts(data_buf.as_ptr() as *const u16, wlen);
        let cmd = String::from_utf16_lossy(cmd_u16)
            .trim_end_matches('\0')
            .to_string();
        out.push((name, cmd));
        index += 1;
    }
    let _ = RegCloseKey(hkey);
    out
}

/// 读 StartupApproved 子键里某值的启用态（键/值缺失→启用）。
unsafe fn approved_state(hive: HKEY, approved_path: &str, value_name: &str) -> bool {
    let path = to_wide(approved_path);
    let mut hkey = HKEY::default();
    if RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
        return true;
    }
    let vname = to_wide(value_name);
    let mut data = [0u8; 32];
    let mut len = data.len() as u32;
    let r = RegQueryValueExW(
        hkey,
        PCWSTR(vname.as_ptr()),
        None,
        None,
        Some(data.as_mut_ptr()),
        Some(&mut len),
    );
    let _ = RegCloseKey(hkey);
    if r != ERROR_SUCCESS {
        return true;
    }
    parse_approved_state(&data[..len as usize])
}

/// 写 StartupApproved（启用/禁用）。键不存在则创建。HKLM 需管理员。
unsafe fn write_approved(
    hive: HKEY,
    approved_path: &str,
    value_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let path = to_wide(approved_path);
    let mut hkey = HKEY::default();
    let r = RegCreateKeyExW(
        hive,
        PCWSTR(path.as_ptr()),
        0,
        PCWSTR::null(),
        REG_OPTION_NON_VOLATILE,
        KEY_SET_VALUE,
        None,
        &mut hkey,
        None,
    );
    if r != ERROR_SUCCESS {
        return Err(format!("打开/创建 StartupApproved 失败: {}", r.0));
    }
    let bytes = encode_approved(enabled);
    let vname = to_wide(value_name);
    let r2 = RegSetValueExW(hkey, PCWSTR(vname.as_ptr()), 0, REG_BINARY, Some(&bytes));
    let _ = RegCloseKey(hkey);
    if r2 != ERROR_SUCCESS {
        return Err(format!("写入失败: {}", r2.0));
    }
    Ok(())
}

fn startup_folder(env_key: &str, tail: &str) -> Option<PathBuf> {
    std::env::var_os(env_key).map(|b| PathBuf::from(b).join(tail))
}

pub fn list_startup() -> Vec<StartupItem> {
    let mut items = Vec::new();
    unsafe {
        for (hive, loc) in [(HKEY_CURRENT_USER, "HKCU-Run"), (HKEY_LOCAL_MACHINE, "HKLM-Run")] {
            for (name, cmd) in read_run_values(hive) {
                let enabled = approved_state(hive, APPROVED_RUN, &name);
                items.push(StartupItem {
                    id: format!("{}|{}", loc, name),
                    name,
                    command: cmd,
                    location: loc.into(),
                    enabled,
                });
            }
        }
    }
    let folders = [
        (
            "User-Folder",
            HKEY_CURRENT_USER,
            startup_folder("APPDATA", "Microsoft\\Windows\\Start Menu\\Programs\\Startup"),
        ),
        (
            "Common-Folder",
            HKEY_LOCAL_MACHINE,
            startup_folder("ProgramData", "Microsoft\\Windows\\Start Menu\\Programs\\Startup"),
        ),
    ];
    for (loc, hive, dir) in folders {
        if let Some(dir) = dir {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let fname = e.file_name().to_string_lossy().to_string();
                    if fname.eq_ignore_ascii_case("desktop.ini") {
                        continue;
                    }
                    let enabled = unsafe { approved_state(hive, APPROVED_FOLDER, &fname) };
                    items.push(StartupItem {
                        id: format!("{}|{}", loc, fname),
                        name: fname.clone(),
                        command: e.path().to_string_lossy().to_string(),
                        location: loc.into(),
                        enabled,
                    });
                }
            }
        }
    }
    items
}

pub fn set_startup_enabled(id: String, enabled: bool) -> ActionResult {
    let (loc, name) = match id.split_once('|') {
        Some((l, n)) => (l, n),
        None => {
            return ActionResult {
                success: false,
                message: "无效的启动项 id".into(),
            }
        }
    };
    let (hive, approved) = match loc {
        "HKCU-Run" => (HKEY_CURRENT_USER, APPROVED_RUN),
        "HKLM-Run" => (HKEY_LOCAL_MACHINE, APPROVED_RUN),
        "User-Folder" => (HKEY_CURRENT_USER, APPROVED_FOLDER),
        "Common-Folder" => (HKEY_LOCAL_MACHINE, APPROVED_FOLDER),
        _ => {
            return ActionResult {
                success: false,
                message: "未知位置".into(),
            }
        }
    };
    match unsafe { write_approved(hive, approved, name, enabled) } {
        Ok(_) => ActionResult {
            success: true,
            message: format!("已{}「{}」", if enabled { "启用" } else { "禁用" }, name),
        },
        Err(e) => ActionResult {
            success: false,
            message: format!("操作失败（HKLM 项需管理员）：{}", e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_approved, parse_approved_state};

    #[test]
    fn approved_state_parsing() {
        assert!(parse_approved_state(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse_approved_state(&[0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!parse_approved_state(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse_approved_state(&[])); // 缺值视为启用
    }

    #[test]
    fn encode_roundtrip() {
        assert_eq!(encode_approved(true)[0], 0x02);
        assert_eq!(encode_approved(false)[0], 0x03);
        assert_eq!(encode_approved(true).len(), 12);
    }
}
```

在 `src-tauri/src/main.rs` 顶部加：

```rust
#[cfg(target_os = "windows")]
mod startup;
```

- [ ] **Step 3: 加命令并注册**

在 `main.rs` 的 `suggest_background` 命令之后加：

```rust
#[cfg(target_os = "windows")]
#[tauri::command]
fn list_startup() -> Vec<startup::StartupItem> {
    startup::list_startup()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_startup_enabled(id: String, enabled: bool) -> process::ActionResult {
    startup::set_startup_enabled(id, enabled)
}
```

在 `generate_handler!` 列表里 `suggest_background` 后加逗号并追加：

```rust
            suggest_background,
            list_startup,
            set_startup_enabled
```

- [ ] **Step 4: 编译 + 测试**

Run: `cd src-tauri && cargo check`
Expected: `Finished`。注册表 API 若签名不符（如 `RegEnumValueW` 的 type 参、`RegCreateKeyExW` 的 options/disposition newtype），按编译错误对照 windows 0.58 修正参数类型，**逻辑与值不变**。

Run: `cd src-tauri && cargo test startup`
Expected: `test result: ok. 2 passed`。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/startup.rs src-tauri/Cargo.toml src-tauri/src/main.rs
git commit -m "feat(startup): 启动项枚举 + StartupApproved 启用/禁用"
```

---

## Task 4: 前端优化中心视图 Optimize.svelte + 路由

**Files:**
- Create: `src/lib/views/Optimize.svelte`
- Modify: `src/App.svelte`（导入 + `toolbox` 路由 + meta 标题）
- Modify: `src/lib/components/Sidebar.svelte`（`toolbox` 标签改名）

**Interfaces:**
- Consumes（后端命令）：`scan_junk()->{categories:[{id,label,bytes,files,needs_admin,available}],total_bytes}`、`clean_junk({ids})->{freed_bytes,items}`、`memory_boost()->{freed_mb,trimmed_count,..}`、`suggest_background()->[{pid,name,mem_mb}]`、`terminate_process({pid})->{success,message}`、`list_startup()->[{id,name,command,location,enabled}]`、`set_startup_enabled({id,enabled})->{success,message}`。
- Consumes（stores）：`elevated`、`relaunchAdmin`、`pushToast`。

- [ ] **Step 1: 写 Optimize.svelte**

创建 `src/lib/views/Optimize.svelte`：

```svelte
<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/tauri";
  import { pushToast, elevated, relaunchAdmin } from "../stores.js";
  import Modal from "../components/Modal.svelte";

  let tab = "optimize"; // optimize | startup

  // 清理
  let scanning = false;
  let report = null;
  let selected = new Set();
  let optimizing = false;
  let result = null;
  let confirmOpen = false;

  // 建议关闭后台
  let bgList = [];
  let bgSelected = new Set();
  let closeOpen = false;

  // 启动项
  let startupItems = [];
  let startupLoading = true;

  const fmtBytes = (n) => {
    if (!n) return "0 B";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };

  function canUse(cat) {
    return cat.available && !(cat.needs_admin && !$elevated);
  }

  async function scan() {
    scanning = true;
    result = null;
    bgList = [];
    try {
      report = await invoke("scan_junk");
      selected = new Set(report.categories.filter(canUse).map((c) => c.id));
    } catch (e) {
      pushToast("扫描失败：" + e, "error");
    } finally {
      scanning = false;
    }
  }

  function toggleCat(id) {
    const s = new Set(selected);
    if (s.has(id)) s.delete(id);
    else s.add(id);
    selected = s;
  }

  $: selectedBytes = report
    ? report.categories.filter((c) => selected.has(c.id)).reduce((a, c) => a + c.bytes, 0)
    : 0;

  async function runOptimize() {
    confirmOpen = false;
    optimizing = true;
    try {
      const ids = [...selected];
      const clean = await invoke("clean_junk", { ids });
      const boost = await invoke("memory_boost");
      result = {
        freed_bytes: clean.freed_bytes,
        freed_mb: boost.freed_mb,
        trimmed_count: boost.trimmed_count,
      };
      bgList = await invoke("suggest_background");
      bgSelected = new Set();
      report = await invoke("scan_junk");
      selected = new Set(report.categories.filter(canUse).map((c) => c.id));
    } catch (e) {
      pushToast("优化失败：" + e, "error");
    } finally {
      optimizing = false;
    }
  }

  function toggleBg(pid) {
    const s = new Set(bgSelected);
    if (s.has(pid)) s.delete(pid);
    else s.add(pid);
    bgSelected = s;
  }

  async function closeSelectedBg() {
    closeOpen = false;
    const pids = [...bgSelected];
    for (const pid of pids) {
      try {
        const r = await invoke("terminate_process", { pid });
        pushToast(r.message, r.success ? "ok" : "error");
      } catch (e) {
        pushToast("结束失败：" + e, "error");
      }
    }
    bgList = bgList.filter((b) => !bgSelected.has(b.pid));
    bgSelected = new Set();
  }

  async function loadStartup() {
    startupLoading = true;
    try {
      startupItems = await invoke("list_startup");
    } catch (e) {
      pushToast("读取启动项失败：" + e, "error");
    } finally {
      startupLoading = false;
    }
  }

  async function toggleStartup(item) {
    try {
      const r = await invoke("set_startup_enabled", { id: item.id, enabled: !item.enabled });
      if (r.success) {
        item.enabled = !item.enabled;
        startupItems = startupItems;
      }
      pushToast(r.message, r.success ? "ok" : "error");
    } catch (e) {
      pushToast("操作失败：" + e, "error");
    }
  }

  const locLabel = (l) =>
    ({
      "HKCU-Run": "用户注册表",
      "HKLM-Run": "系统注册表",
      "User-Folder": "用户启动文件夹",
      "Common-Folder": "公共启动文件夹",
    })[l] || l;

  onMount(loadStartup);
</script>

{#if !$elevated}
  <div class="admin-banner">
    <span>部分操作需要管理员权限（系统垃圾清理 / 系统注册表启动项 / 裁剪系统进程）。</span>
    <button class="primary" on:click={relaunchAdmin}>以管理员重启</button>
  </div>
{/if}

<div class="tabs" role="tablist">
  <button class="tab" class:active={tab === "optimize"} on:click={() => (tab = "optimize")}>一键优化</button>
  <button class="tab" class:active={tab === "startup"} on:click={() => (tab = "startup")}>
    启动项 <span class="tab-badge">{startupItems.length}</span>
  </button>
</div>

{#if tab === "optimize"}
  <section class="opt">
    {#if !report}
      <div class="scan-intro">
        <p class="muted">扫描临时文件、缓存、回收站等可清理项，并可一键释放内存。</p>
        <button class="primary big" on:click={scan} disabled={scanning}>
          {scanning ? "扫描中…" : "扫描垃圾"}
        </button>
      </div>
    {:else}
      <div class="summary">
        <div class="gauge">
          <div class="gauge-num mono">{fmtBytes(selectedBytes)}</div>
          <div class="gauge-label muted">已选可清理</div>
        </div>
        <div class="summary-actions">
          <button class="ghost" on:click={scan} disabled={scanning}>{scanning ? "扫描中…" : "重新扫描"}</button>
          <button class="primary" on:click={() => (confirmOpen = true)} disabled={optimizing || selected.size === 0}>
            {optimizing ? "优化中…" : "一键优化"}
          </button>
        </div>
      </div>

      <div class="cats">
        {#each report.categories as c (c.id)}
          <label class="cat" class:off={!canUse(c)}>
            <input type="checkbox" checked={selected.has(c.id)} disabled={!canUse(c)} on:change={() => toggleCat(c.id)} />
            <span class="cat-label">
              {c.label}
              {#if c.needs_admin}<span class="lock" title="需管理员">🔒</span>{/if}
              {#if !c.available}<span class="muted">（不可用）</span>{/if}
            </span>
            <span class="cat-size mono">{fmtBytes(c.bytes)}</span>
          </label>
        {/each}
      </div>

      {#if result}
        <div class="result">
          <div class="res-item"><span class="muted">释放磁盘</span><b class="mono">{fmtBytes(result.freed_bytes)}</b></div>
          <div class="res-item"><span class="muted">释放内存</span><b class="mono">{result.freed_mb.toFixed(0)} MB</b></div>
          <div class="res-item"><span class="muted">裁剪进程</span><b class="mono">{result.trimmed_count}</b></div>
        </div>

        {#if bgList.length > 0}
          <div class="bg">
            <div class="bg-head">
              <h3>建议关闭的后台进程</h3>
              <button class="danger" on:click={() => (closeOpen = true)} disabled={bgSelected.size === 0}>
                结束所选 ({bgSelected.size})
              </button>
            </div>
            <div class="bg-list">
              {#each bgList as b (b.pid)}
                <label class="bg-item">
                  <input type="checkbox" checked={bgSelected.has(b.pid)} on:change={() => toggleBg(b.pid)} />
                  <span class="bg-name">{b.name}</span>
                  <span class="muted mono">PID {b.pid}</span>
                  <span class="bg-mem mono">{b.mem_mb.toFixed(0)} MB</span>
                </label>
              {/each}
            </div>
          </div>
        {/if}
      {/if}
    {/if}
  </section>
{:else}
  <div class="table-wrap">
    <table>
      <thead>
        <tr><th class="col-name">名称</th><th>位置</th><th class="col-cmd">命令</th><th class="col-sw">状态</th></tr>
      </thead>
      <tbody>
        {#if startupLoading}
          <tr><td colspan="4" class="empty">加载中…</td></tr>
        {:else if startupItems.length === 0}
          <tr><td colspan="4" class="empty">无启动项</td></tr>
        {:else}
          {#each startupItems as item (item.id)}
            <tr>
              <td class="col-name" title={item.name}>{item.name}</td>
              <td>{locLabel(item.location)}</td>
              <td class="col-cmd mono" title={item.command}>{item.command}</td>
              <td class="col-sw">
                <button class="switch" class:on={item.enabled} on:click={() => toggleStartup(item)} aria-pressed={item.enabled}>
                  {item.enabled ? "已启用" : "已禁用"}
                </button>
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
{/if}

<Modal open={confirmOpen} title="确认一键优化" on:close={() => (confirmOpen = false)}>
  <p>将清理选中的 {selected.size} 个分类（约 {fmtBytes(selectedBytes)}），并裁剪进程工作集释放内存。</p>
  <p class="muted">清理为删除操作、不可撤销；被占用的文件会自动跳过。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (confirmOpen = false)}>取消</button>
    <button class="primary" on:click={runOptimize}>确认优化</button>
  </div>
</Modal>

<Modal open={closeOpen} title="结束所选进程" on:close={() => (closeOpen = false)}>
  <p>将结束选中的 {bgSelected.size} 个进程，未保存数据可能丢失。</p>
  <div class="modal-actions">
    <button class="ghost" on:click={() => (closeOpen = false)}>取消</button>
    <button class="danger" on:click={closeSelectedBg}>确认结束</button>
  </div>
</Modal>

<style>
  .admin-banner {
    display: flex;
    align-items: center;
    gap: var(--sp-4);
    flex-wrap: wrap;
    padding: 10px 14px;
    margin-bottom: var(--sp-4);
    border: 1px solid rgba(245, 158, 11, 0.35);
    background: rgba(245, 158, 11, 0.1);
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: #fcd34d;
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border);
  }
  .tab {
    border: none;
    background: transparent;
    color: var(--text-muted);
    font-family: inherit;
    font-size: 14px;
    font-weight: 500;
    padding: 10px 16px;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .tab:hover { color: var(--text); }
  .tab.active { color: var(--text); border-bottom-color: var(--accent); }
  .tab-badge {
    font-size: 11px;
    color: var(--text-muted);
    background: var(--surface-2);
    padding: 1px 7px;
    border-radius: 999px;
    font-variant-numeric: tabular-nums;
  }

  .scan-intro {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-4);
    padding: var(--sp-6);
    text-align: center;
  }
  .summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    padding: var(--sp-4) var(--sp-6);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    margin-bottom: var(--sp-4);
  }
  .gauge-num { font-size: 32px; font-weight: 700; font-variant-numeric: tabular-nums; }
  .gauge-label { font-size: 12px; }
  .summary-actions { display: flex; gap: var(--sp-2); }

  .cats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: var(--sp-2);
  }
  .cat {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    cursor: pointer;
    font-size: 13px;
  }
  .cat.off { opacity: 0.5; cursor: default; }
  .cat input { accent-color: var(--accent); }
  .cat-label { flex: 1; }
  .cat-size { color: var(--text-muted); }
  .lock { margin-left: 4px; }

  .result {
    display: flex;
    gap: var(--sp-6);
    margin: var(--sp-4) 0;
    padding: var(--sp-4);
    border: 1px solid rgba(34, 197, 94, 0.35);
    background: rgba(34, 197, 94, 0.08);
    border-radius: var(--radius);
  }
  .res-item { display: flex; flex-direction: column; gap: 2px; }
  .res-item b { font-size: 20px; }

  .bg { margin-top: var(--sp-4); }
  .bg-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: var(--sp-2); }
  .bg-head h3 { margin: 0; font-size: 14px; }
  .bg-list { display: flex; flex-direction: column; gap: 6px; }
  .bg-item {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    cursor: pointer;
    font-size: 13px;
  }
  .bg-item input { accent-color: var(--accent); }
  .bg-name { flex: 1; font-weight: 500; }
  .bg-mem { color: var(--text-muted); }

  .table-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface);
  }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  thead th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    text-align: left;
    padding: 10px 14px;
    font-weight: 500;
    color: var(--text-muted);
    white-space: nowrap;
  }
  tbody { display: block; max-height: calc(100vh - 230px); overflow-y: auto; }
  thead, tbody tr { display: table; width: 100%; table-layout: fixed; }
  td { padding: 8px 14px; border-top: 1px solid var(--border); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  tbody tr:hover { background: var(--surface-2); }
  .col-sw { width: 110px; text-align: right; }
  .empty { text-align: center; color: var(--text-muted); padding: 40px; }

  .mono { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .muted { color: var(--text-muted); }

  .primary {
    border: none;
    background: linear-gradient(135deg, var(--accent), #7c3aed);
    color: #fff;
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .primary.big { font-size: 15px; padding: 12px 28px; }
  .primary:disabled { opacity: 0.6; cursor: default; }
  .ghost {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    padding: 8px 16px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ghost:hover { background: var(--surface-2); }
  .danger {
    border: 1px solid rgba(239, 68, 68, 0.4);
    background: transparent;
    color: var(--danger);
    font-family: inherit;
    font-size: 12px;
    padding: 6px 12px;
    border-radius: 8px;
    cursor: pointer;
  }
  .danger:hover { background: rgba(239, 68, 68, 0.12); }
  .danger:disabled { opacity: 0.5; cursor: default; }
  .switch {
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-muted);
    font-family: inherit;
    font-size: 12px;
    padding: 4px 12px;
    border-radius: 999px;
    cursor: pointer;
  }
  .switch.on { color: var(--ok); border-color: rgba(34, 197, 94, 0.4); background: rgba(34, 197, 94, 0.12); }
  .modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); margin-top: var(--sp-3); }
</style>
```

- [ ] **Step 2: 路由到 Optimize**

在 `src/App.svelte` 的视图导入区（`import Placeholder ...` 附近）加：

```js
  import Optimize from "./lib/views/Optimize.svelte";
```

在模板的视图路由里，把 `{:else if current === "disk"}` 那条之后、`{:else if current === "about"}` 之前，插入一条分支：

```svelte
      {:else if current === "toolbox"}
        <Optimize />
```

把 `meta` 里 `toolbox` 的 `title` 由 `"工具箱"` 改为 `"优化加速"`（`plan` 字段保留无妨，toolbox 已不再走 Placeholder）。

- [ ] **Step 3: 侧栏标签改名**

在 `src/lib/components/Sidebar.svelte` 的 `items` 数组里，把 `{ id: "toolbox", label: "工具箱", icon: "terminal" }` 的 `label` 改为 `"优化加速"`（id/icon 不变）。

- [ ] **Step 4: 构建检查**

Run（仓库根）: `npm run build`
Expected: 成功（`built in ...`），无 Svelte 编译错误、无 "Unused CSS selector" 警告。若有未用选择器警告，说明 class 与样式不匹配——修正匹配，勿删样式。

- [ ] **Step 5: Commit**

```bash
git add src/lib/views/Optimize.svelte src/App.svelte src/lib/components/Sidebar.svelte
git commit -m "feat(optimize): 优化中心视图(一键清理+加速/启动项) + 路由"
```

---

## Task 5: 提权端到端实测（用户配合）

**Files:** 无（端到端验证）。

后端 Windows API（SH*/EmptyWorkingSet/注册表写）无法在非提权 CI 验证，需用户提权实测。

- [ ] **Step 1: 关闭实例并重编 release**

关闭所有 Win-Top 窗口（含提权实例），然后：
```
powershell Stop-Process -Name win-top -Force
cargo build --release --features custom-protocol --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 2: 以管理员运行**

右键 `src-tauri\target\release\win-top.exe` →「以管理员身份运行」，进入「优化加速」。

- [ ] **Step 3: 验证一键优化**

「扫描垃圾」→ 应列出各分类体积（系统类不再灰显）；勾选 →「一键优化」确认 → 结果卡显示释放磁盘/内存/裁剪进程数（磁盘数应与各 Temp/缓存清空大致相符；内存释放数可能小，属正常）；下方出现「建议关闭的后台进程」候选。勾选某个 →「结束所选」确认 → 对应进程消失。

- [ ] **Step 4: 验证启动项**

切「启动项」→ 应列出 Run 键 + 启动文件夹项；对某项点「已启用」切到「已禁用」→ 打开**任务管理器→启动**对照该项状态应同步为「已禁用」；重启后该项不自启即验证成功。HKLM/系统项在非提权时切换应提示需管理员。

- [ ] **Step 5: 验证降级路径（可选）**

普通权限运行：系统类清理分类灰显+锁标、系统注册表启动项开关失败提示需管理员，用户级清理/HKCU 启动项/自身进程裁剪照常可用，无报错。

- [ ] **Step 6: 用户截图确认 GUI**

WebView2 无法 attach，最终渲染由用户截图确认。

- [ ] **Step 7:** 无新代码改动则无需 commit；若实测中按编译/行为修正了 API 细节，单独 commit。

---

## 自检小结

- **Spec 覆盖：** 清理 8 分类+回收站+安全容错→Task 1；内存裁剪+后台建议→Task 2；启动项枚举+StartupApproved 开关→Task 3；前端两标签+一键优化流程+启动项表+提权门禁→Task 4；提权实测→Task 5。Cargo features（ProcessStatus/Registry）随各自模块加入。
- **无占位符：** 各步含完整代码与确切命令。
- **类型一致：** 命令名与返回结构（`scan_junk`/`clean_junk`/`memory_boost`/`suggest_background`/`list_startup`/`set_startup_enabled`）在后端定义与前端 `invoke` 调用、字段名（`freed_bytes`/`freed_mb`/`trimmed_count`/`id`/`enabled` 等）逐项对齐；`ActionResult` 复用 `process::ActionResult`。

