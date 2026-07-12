//! 卸载执行器。
//!
//! 两步、诚实、可回滚：
//!   ① 运行厂商卸载器（或 msiexec /x）——**不可逆**，与手动卸载等价，自行 UAC 提权。
//!   ② 残留清理——**可撤销**：文件/目录移入受控隔离区（同卷 rename，可一键还原）、
//!      注册表键先导出 .reg 再删除。撤销只还原第②步删掉的东西，不承诺让软件复活如初。
//!
//! 双重安全闸（防误删）：
//!   - 待删目标必须**在后端刚解析出的 footprint 产物集合里**（绝不信任前端任意路径）；
//!   - 且必须通过受保护路径 denylist（系统目录 / 盘根 / 用户特殊文件夹一律拒绝）。
//! v1 不自动删除服务/启动项（更危险、且厂商卸载器通常已处理），只在 footprint 里展示。

use std::fs;
use std::iter::once;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::{app_inventory, footprint};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize, Default)]
pub struct RemovalResult {
    pub undo_token: String,
    pub freed_bytes: u64,
    pub removed: u32,
    pub skipped: u32,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct ManifestItem {
    kind: String, // fs | reg
    original: String,
    stored: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    app_id: String,
    app_name: String,
    items: Vec<ManifestItem>,
}

// ─── 厂商卸载器 ───────────────────────────────────────────────────────

/// 把卸载命令拆成 (exe, params)。支持带引号路径、裸 .exe、msiexec 形式。
fn split_command(cmd: &str) -> (String, String) {
    let c = cmd.trim();
    if let Some(rest) = c.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    let lower = c.to_lowercase();
    if let Some(pos) = lower.find(".exe") {
        return (c[..pos + 4].to_string(), c[pos + 4..].trim().to_string());
    }
    (c.to_string(), String::new())
}

fn launch(exe: &str, params: &str) -> Result<String, String> {
    let exe_w: Vec<u16> = exe.encode_utf16().chain(once(0)).collect();
    let par_w: Vec<u16> = params.encode_utf16().chain(once(0)).collect();
    let r = unsafe {
        // 本函数在 spawn_blocking 的 worker 线程执行（避免占用 UI 主线程冻结 WebView）。
        // worker 线程默认未初始化 COM；ShellExecuteW 走 shell/提权路径时需要 COM，先初始化。
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let h = ShellExecuteW(
            HWND::default(),
            PCWSTR::null(), // 默认动作，卸载器自身 manifest 决定是否 UAC 提权
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(par_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if com.is_ok() {
            CoUninitialize();
        }
        h
    };
    if (r.0 as isize) <= 32 {
        Err("启动卸载程序失败（可能被 UAC 取消或命令无效）".into())
    } else {
        Ok("已启动卸载程序；完成后点「重新扫描产物」查看残留".into())
    }
}

/// 运行某应用的厂商卸载器（不等待 GUI 卸载器退出）。
pub fn run_uninstaller(app_id: &str) -> Result<String, String> {
    let app = app_inventory::list_installed_apps()
        .into_iter()
        .find(|a| a.id == app_id)
        .ok_or_else(|| "未找到应用".to_string())?;

    let (exe, params) = if !app.quiet_uninstall_string.is_empty() {
        split_command(&app.quiet_uninstall_string)
    } else if !app.uninstall_string.is_empty() {
        split_command(&app.uninstall_string)
    } else if app.is_msi && !app.product_code.is_empty() {
        ("msiexec.exe".to_string(), format!("/x {}", app.product_code))
    } else {
        return Err("该应用没有可用的卸载命令（僵尸条目）。v1 暂不支持强制卸载。".into());
    };
    launch(&exe, &params)
}

// ─── 安全闸 ───────────────────────────────────────────────────────────

fn env_lower(k: &str) -> String {
    std::env::var(k).unwrap_or_default().to_lowercase()
}

/// 受保护路径：系统目录 / 盘根 / 用户特殊文件夹根——一律拒绝删除。
fn is_protected(path: &str) -> bool {
    let p = path.trim().trim_end_matches('\\').to_lowercase().replace('/', "\\");
    if p.len() < 4 {
        return true;
    }
    // 盘根 c:\ 之类
    if p.len() <= 3 && p.as_bytes().get(1) == Some(&b':') {
        return true;
    }
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "c:\\windows".into()).to_lowercase();
    let up = env_lower("USERPROFILE");
    let mut protected = vec![
        windir.clone(),
        format!("{}\\system32", windir),
        env_lower("ProgramFiles"),
        env_lower("ProgramFiles(x86)"),
        env_lower("ProgramData"),
        env_lower("PUBLIC"),
        env_lower("APPDATA"),
        env_lower("LOCALAPPDATA"),
        up.clone(),
    ];
    if !up.is_empty() {
        for tail in ["documents", "desktop", "downloads", "pictures", "videos", "music"] {
            protected.push(format!("{}\\{}", up, tail));
        }
    }
    // 精确命中受保护根
    if protected.iter().any(|r| !r.is_empty() && p == *r) {
        return true;
    }
    // p 是 windir 的祖先（如 c:\）
    if !windir.is_empty() && windir.starts_with(&format!("{}\\", p)) {
        return true;
    }
    false
}

// ─── 残留清理（可回滚） ───────────────────────────────────────────────

fn quarantine_base() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default())
        .join("WinTop")
        .join("quarantine")
}

fn new_token() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("wintop_{}", ms)
}

fn reg_export(key: &str, file: &Path) -> bool {
    Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["export", key, &file.to_string_lossy(), "/y"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn reg_delete(key: &str) -> bool {
    Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["delete", key, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn reg_import(file: &Path) -> bool {
    Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["import", &file.to_string_lossy()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 删除用户确认的残留（文件/目录 -> 隔离区；注册表键 -> 导出后删除）。可用 undo_token 撤销。
pub fn remove_residue(app_id: &str, targets: Vec<String>) -> RemovalResult {
    let mut warnings = Vec::new();

    // 后端独立重解析（绝不信任前端路径）
    let fp = match footprint::resolve_by_id(app_id) {
        Ok(f) => f,
        Err(e) => {
            return RemovalResult {
                skipped: targets.len() as u32,
                warnings: vec![format!("解析失败：{}", e)],
                ..Default::default()
            }
        }
    };
    let mut valid: std::collections::HashMap<String, (footprint::ArtifactKind, u64)> =
        std::collections::HashMap::new();
    for a in &fp.artifacts {
        valid.insert(a.path.to_lowercase(), (a.kind, a.size));
    }

    let token = new_token();
    let qdir = quarantine_base().join(&token);
    if fs::create_dir_all(&qdir).is_err() {
        return RemovalResult {
            skipped: targets.len() as u32,
            warnings: vec!["无法创建隔离目录".into()],
            ..Default::default()
        };
    }

    let mut items: Vec<ManifestItem> = Vec::new();
    let mut freed = 0u64;
    let mut removed = 0u32;
    let mut skipped = 0u32;
    let mut n = 0u32;

    for t in &targets {
        let (kind, size) = match valid.get(&t.to_lowercase()) {
            Some(v) => *v,
            None => {
                warnings.push(format!("跳过（不在解析结果内，安全拦截）：{}", t));
                skipped += 1;
                continue;
            }
        };
        match kind {
            footprint::ArtifactKind::Dir => {
                if is_protected(t) {
                    warnings.push(format!("跳过（受保护路径）：{}", t));
                    skipped += 1;
                    continue;
                }
                let base = Path::new(t)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "item".into());
                let dest = qdir.join(format!("{}_{}", n, base));
                n += 1;
                match fs::rename(t, &dest) {
                    Ok(_) => {
                        items.push(ManifestItem {
                            kind: "fs".into(),
                            original: t.clone(),
                            stored: dest.to_string_lossy().into(),
                        });
                        freed += size;
                        removed += 1;
                    }
                    Err(e) => {
                        warnings.push(format!("移动失败，已跳过（未删除）：{}（{}）", t, e));
                        skipped += 1;
                    }
                }
            }
            footprint::ArtifactKind::RegKey => {
                let file = qdir.join(format!("reg_{}.reg", n));
                n += 1;
                if reg_export(t, &file) && reg_delete(t) {
                    items.push(ManifestItem {
                        kind: "reg".into(),
                        original: t.clone(),
                        stored: file.to_string_lossy().into(),
                    });
                    removed += 1;
                } else {
                    warnings.push(format!("注册表键处理失败，已跳过：{}", t));
                    skipped += 1;
                }
            }
            _ => {
                warnings.push(format!("跳过（v1 不自动删除服务/启动项）：{}", t));
                skipped += 1;
            }
        }
    }

    let manifest = Manifest {
        app_id: app_id.to_string(),
        app_name: fp.app_name.clone(),
        items,
    };
    let _ = fs::write(
        qdir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    );

    RemovalResult {
        undo_token: token,
        freed_bytes: freed,
        removed,
        skipped,
        warnings,
    }
}

/// 撤销一次残留清理：文件移回原处、注册表键重新导入。
pub fn undo_removal(undo_token: &str) -> Result<String, String> {
    let qdir = quarantine_base().join(undo_token);
    let bytes = fs::read(qdir.join("manifest.json")).map_err(|_| "找不到隔离清单".to_string())?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;

    let mut restored = 0u32;
    let mut failed = 0u32;
    for it in &manifest.items {
        match it.kind.as_str() {
            "fs" => {
                if let Some(parent) = Path::new(&it.original).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if fs::rename(&it.stored, &it.original).is_ok() {
                    restored += 1;
                } else {
                    failed += 1;
                }
            }
            "reg" => {
                if reg_import(Path::new(&it.stored)) {
                    restored += 1;
                } else {
                    failed += 1;
                }
            }
            _ => {}
        }
    }
    Ok(format!(
        "已还原「{}」的 {} 项残留{}",
        manifest.app_name,
        restored,
        if failed > 0 {
            format!("，{} 项失败", failed)
        } else {
            String::new()
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_splitting() {
        assert_eq!(
            split_command("\"C:\\A B\\uninst.exe\" /S"),
            ("C:\\A B\\uninst.exe".into(), "/S".into())
        );
        assert_eq!(
            split_command("MsiExec.exe /X{0F1B-...}"),
            ("MsiExec.exe".into(), "/X{0F1B-...}".into())
        );
        assert_eq!(
            split_command("C:\\App\\unins000.exe"),
            ("C:\\App\\unins000.exe".into(), String::new())
        );
    }

    #[test]
    fn protected_paths() {
        assert!(is_protected("C:\\"));
        assert!(is_protected("c:\\windows"));
        assert!(is_protected("C:\\Windows\\System32"));
        // 深层的应用目录不受保护
        assert!(!is_protected("C:\\Users\\me\\AppData\\Local\\SomeVendorApp"));
        assert!(!is_protected("C:\\Program Files\\SomeVendor\\App"));
    }

    #[test]
    fn removal_rejects_unresolved_target() {
        // 传一个不存在的 app_id：解析失败，所有目标应被安全跳过、零删除。
        let res = remove_residue("HKLM|__nonexistent__", vec!["C:\\Windows".into()]);
        assert_eq!(res.removed, 0);
        assert!(res.undo_token.is_empty());
    }
}
