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

#[derive(Serialize)]
pub struct StaleEntryResult {
    pub undo_token: String,
    pub key: String,
    pub app_name: String,
}

#[derive(Serialize, Deserialize)]
struct ManifestItem {
    kind: String, // fs | reg
    original: String,
    stored: String,
    /// 注册表导出/导入使用的视图（/reg:32 或 /reg:64）；旧清单缺省为默认视图。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    view: Option<String>,
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

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum UninstallMode {
    Interactive,
    Quiet,
    Unavailable,
}

#[derive(Serialize, Debug)]
pub struct UninstallCapability {
    pub mode: UninstallMode,
    pub message: String,
    /// 卸载程序或安装目录均已缺失，属于残留注册项；可安全移除登记。
    pub stale_entry: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct SelectedCommand {
    exe: String,
    params: String,
    mode: UninstallMode,
}

/// 卸载命令中的可执行文件是否真实存在（支持 %VAR% 展开；msiexec 视为系统可用）。
fn command_exe_exists(exe: &str) -> bool {
    let expanded = app_inventory::expand_env(exe.trim());
    let lower = expanded.to_lowercase();
    if lower == "msiexec.exe" || lower == "msiexec" {
        return true;
    }
    let path = Path::new(&expanded);
    if path.is_absolute() {
        return path.is_file();
    }
    // 裸文件名（rundll32.exe / cmd.exe 等）：只接受 System32 中确实存在的文件，
    // 不按相对路径猜测，避免把无效登记误判为可卸载。
    std::env::var_os("WINDIR")
        .map(|windir| PathBuf::from(windir).join("System32").join(&expanded).is_file())
        .unwrap_or(false)
}

fn capability_for(app: &app_inventory::AppEntry) -> UninstallCapability {
    // 交互式卸载程序存在 → 正常交互卸载
    if !app.uninstall_string.is_empty() {
        let (exe, _) = split_command(&app.uninstall_string);
        if command_exe_exists(&exe) {
            return UninstallCapability {
                mode: UninstallMode::Interactive,
                message: "将打开厂商卸载器，由你确认卸载选项。".into(),
                stale_entry: false,
            };
        }
    }
    // MSI 产品码可交给 Windows Installer
    if app.is_msi && !app.product_code.is_empty() {
        return UninstallCapability {
            mode: UninstallMode::Interactive,
            message: "将通过 Windows Installer 卸载该应用。".into(),
            stale_entry: false,
        };
    }
    // 交互式缺失但静默卸载程序存在 → 仍需用户明确确认
    if !app.quiet_uninstall_string.is_empty() {
        let (exe, _) = split_command(&app.quiet_uninstall_string);
        if command_exe_exists(&exe) {
            return UninstallCapability {
                mode: UninstallMode::Quiet,
                message: if app.uninstall_string.is_empty() {
                    "该应用只提供静默卸载，确认后可能不显示厂商界面。".into()
                } else {
                    "交互式卸载程序不存在，仅提供静默卸载。".into()
                },
                stale_entry: false,
            };
        }
    }

    // 没有任何可执行命令：判断是否连安装目录也已消失，属于残留登记。
    let command = if !app.uninstall_string.is_empty() {
        Some(app.uninstall_string.as_str())
    } else if !app.quiet_uninstall_string.is_empty() {
        Some(app.quiet_uninstall_string.as_str())
    } else {
        None
    };
    let missing_exe = command
        .map(|cmd| split_command(cmd).0)
        .filter(|exe| {
            let expanded = app_inventory::expand_env(exe);
            let path = Path::new(&expanded);
            path.is_absolute() && !path.is_file()
        });
    let install_missing = !app.install_location.trim().is_empty()
        && !Path::new(&app_inventory::expand_env(&app.install_location)).exists();
    let stale_entry = install_missing || missing_exe.is_some();
    let detail = match missing_exe {
        Some(exe) => format!("卸载程序不存在：{}", exe),
        None => "未提供可用的卸载命令".to_string(),
    };
    let message = if stale_entry {
        let install_note = if install_missing { "，安装目录也不存在" } else { "" };
        format!(
            "{detail}{install_note}；该条目可能是残留登记，可清理无效的卸载登记。"
        )
    } else {
        format!("{detail}；无法自动启动卸载。")
    };
    UninstallCapability {
        mode: UninstallMode::Unavailable,
        message,
        stale_entry,
    }
}

fn select_uninstall_command(
    app: &app_inventory::AppEntry,
    allow_quiet: bool,
) -> Result<SelectedCommand, String> {
    if !app.uninstall_string.is_empty() {
        let (exe, params) = split_command(&app.uninstall_string);
        return Ok(SelectedCommand {
            exe,
            params,
            mode: UninstallMode::Interactive,
        });
    }
    if app.is_msi && !app.product_code.is_empty() {
        return Ok(SelectedCommand {
            exe: "msiexec.exe".into(),
            params: format!("/x {}", app.product_code),
            mode: UninstallMode::Interactive,
        });
    }
    if !app.quiet_uninstall_string.is_empty() {
        if !allow_quiet {
            return Err("该应用只提供静默卸载，需要明确确认".into());
        }
        let (exe, params) = split_command(&app.quiet_uninstall_string);
        return Ok(SelectedCommand {
            exe,
            params,
            mode: UninstallMode::Quiet,
        });
    }
    Err("该应用没有可用的卸载命令（僵尸条目）".into())
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
        Ok("已启动卸载程序，正在等待系统确认卸载结果".into())
    }
}

pub fn uninstall_capability(app_id: &str) -> Result<UninstallCapability, String> {
    let app = app_inventory::list_installed_apps()
        .into_iter()
        .find(|a| a.id == app_id)
        .ok_or_else(|| "未找到应用".to_string())?;
    Ok(capability_for(&app))
}

/// 运行某应用的厂商卸载器（不等待 GUI 卸载器退出）。
pub fn start_uninstaller(app_id: &str, allow_quiet: bool) -> Result<String, String> {
    let app = app_inventory::list_installed_apps()
        .into_iter()
        .find(|a| a.id == app_id)
        .ok_or_else(|| "未找到应用".to_string())?;
    let mut selected = select_uninstall_command(&app, allow_quiet)?;
    if !command_exe_exists(&selected.exe) {
        // 交互式卸载程序可能已被删除；若静默卸载程序仍存在，允许改用静默方式。
        if !app.quiet_uninstall_string.is_empty() {
            let (exe, params) = split_command(&app.quiet_uninstall_string);
            if command_exe_exists(&exe) {
                if !allow_quiet {
                    return Err("交互式卸载程序不存在，仅提供静默卸载，需要明确确认".into());
                }
                selected = SelectedCommand {
                    exe,
                    params,
                    mode: UninstallMode::Quiet,
                };
            }
        }
    }
    if !command_exe_exists(&selected.exe) {
        return Err(format!(
            "卸载程序不存在：{}。该条目可能是残留登记，可在应用详情中清理。",
            selected.exe
        ));
    }
    launch(&selected.exe, &selected.params)
}

pub fn is_app_installed(app_id: &str) -> bool {
    app_inventory::list_installed_apps()
        .iter()
        .any(|app| app.id == app_id)
}

fn ensure_app_removed(installed: bool) -> Result<(), &'static str> {
    if installed {
        Err("应用仍处于安装状态，不能清理产物")
    } else {
        Ok(())
    }
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

fn reg_run(args: &[String]) -> bool {
    Command::new("reg")
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn reg_export_view(key: &str, file: &Path, view: Option<&str>) -> bool {
    let mut args = vec![
        "export".to_string(),
        key.to_string(),
        file.to_string_lossy().to_string(),
        "/y".to_string(),
    ];
    if let Some(view) = view {
        args.push(view.to_string());
    }
    reg_run(&args)
}

fn reg_delete_view(key: &str, view: Option<&str>) -> bool {
    let mut args = vec!["delete".to_string(), key.to_string(), "/f".to_string()];
    if let Some(view) = view {
        args.push(view.to_string());
    }
    reg_run(&args)
}

fn reg_import_view(file: &Path, view: Option<&str>) -> bool {
    let mut args = vec!["import".to_string(), file.to_string_lossy().to_string()];
    if let Some(view) = view {
        args.push(view.to_string());
    }
    reg_run(&args)
}

fn reg_export(key: &str, file: &Path) -> bool {
    reg_export_view(key, file, None)
}

fn reg_delete(key: &str) -> bool {
    reg_delete_view(key, None)
}

/// 删除用户确认的残留（文件/目录 -> 隔离区；注册表键 -> 导出后删除）。可用 undo_token 撤销。
pub fn remove_residue(app_id: &str, targets: Vec<String>) -> RemovalResult {
    let mut warnings = Vec::new();

    if let Err(message) = ensure_app_removed(is_app_installed(app_id)) {
        return RemovalResult {
            skipped: targets.len() as u32,
            warnings: vec![message.into()],
            ..Default::default()
        };
    }

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
    let mut valid: std::collections::HashMap<
        String,
        (footprint::ArtifactKind, u64, footprint::Confidence),
    > = std::collections::HashMap::new();
    for a in &fp.artifacts {
        valid.insert(a.path.to_lowercase(), (a.kind, a.size, a.confidence));
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
        let (kind, size, confidence) = match valid.get(&t.to_lowercase()) {
            Some(v) => *v,
            None => {
                warnings.push(format!("跳过（不在解析结果内，安全拦截）：{}", t));
                skipped += 1;
                continue;
            }
        };
        // 弱匹配项（仅名称片段相似）不允许自动清理，必须由用户手动定位确认。
        if confidence == footprint::Confidence::Low {
            warnings.push(format!("跳过（弱匹配项，归属不确定，请手动确认）：{}", t));
            skipped += 1;
            continue;
        }
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
                            view: None,
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
                        view: None,
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

/// 清理“卸载程序/安装目录都已不存在”的残留注册项。
/// 只删除 ARP 登记本身，不动任何文件；删除前导出 .reg 备份，可通过 undo_removal 恢复。
pub fn remove_stale_uninstall_entry(app_id: &str) -> Result<StaleEntryResult, String> {
    let app = app_inventory::list_installed_apps()
        .into_iter()
        .find(|a| a.id == app_id)
        .ok_or_else(|| "未找到应用".to_string())?;
    let capability = capability_for(&app);
    if !capability.stale_entry {
        return Err("该应用仍有可用的卸载程序或安装目录，不能按无效登记清理".into());
    }

    let (source, subkey) = app
        .id
        .split_once('|')
        .ok_or_else(|| "无法识别注册表位置".to_string())?;
    if subkey.is_empty() || subkey.contains('\\') || subkey.contains('/') {
        return Err("注册表子键不合法，已拒绝操作".into());
    }
    let (root, view) = match source {
        "HKCU" => (
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "/reg:64",
        ),
        "HKLM" => (
            "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "/reg:64",
        ),
        "HKLM-WOW" => (
            "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            "/reg:32",
        ),
        _ => return Err("未知的注册表来源".into()),
    };
    let key = format!("{}\\{}", root, subkey);

    let token = new_token();
    let qdir = quarantine_base().join(&token);
    fs::create_dir_all(&qdir).map_err(|e| format!("无法创建隔离目录：{}", e))?;
    let backup = qdir.join("reg_0.reg");
    if !reg_export_view(&key, &backup, Some(view)) {
        return Err("导出注册表备份失败，未做任何修改".into());
    }
    if !reg_delete_view(&key, Some(view)) {
        return Err("删除无效卸载登记失败（备份已保留）".into());
    }

    let manifest = Manifest {
        app_id: app.id.clone(),
        app_name: app.name.clone(),
        items: vec![ManifestItem {
            kind: "reg".into(),
            original: key.clone(),
            stored: backup.to_string_lossy().into(),
            view: Some(view.to_string()),
        }],
    };
    fs::write(
        qdir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("写入隔离清单失败：{}", e))?;

    Ok(StaleEntryResult {
        undo_token: token,
        key,
        app_name: app.name,
    })
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
                if reg_import_view(Path::new(&it.stored), it.view.as_deref()) {
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

    fn test_app(uninstall: &str, quiet: &str, is_msi: bool, product_code: &str) -> app_inventory::AppEntry {
        app_inventory::AppEntry {
            id: "HKLM|test".into(),
            name: "Test App".into(),
            publisher: "Test Publisher".into(),
            version: "1.0".into(),
            install_date: String::new(),
            install_location: String::new(),
            estimated_size_kb: 0,
            uninstall_string: uninstall.into(),
            quiet_uninstall_string: quiet.into(),
            display_icon: String::new(),
            is_msi,
            product_code: product_code.into(),
            arch: "x64".into(),
            source: "HKLM".into(),
        }
    }

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
    fn interactive_command_wins_over_quiet() {
        let app = test_app("normal.exe /remove", "quiet.exe /S", false, "");
        let selected = select_uninstall_command(&app, false).unwrap();
        assert_eq!(selected.exe, "normal.exe");
        assert_eq!(selected.mode, UninstallMode::Interactive);
    }

    #[test]
    fn quiet_only_requires_explicit_permission() {
        let app = test_app("", "quiet.exe /S", false, "");
        assert!(select_uninstall_command(&app, false).is_err());
        assert_eq!(
            select_uninstall_command(&app, true).unwrap().mode,
            UninstallMode::Quiet
        );
    }

    #[test]
    fn msi_fallback_is_interactive() {
        let app = test_app("", "", true, "{PRODUCT-CODE}");
        let selected = select_uninstall_command(&app, false).unwrap();
        assert_eq!(selected.exe.to_lowercase(), "msiexec.exe");
        assert_eq!(selected.params, "/x {PRODUCT-CODE}");
        assert_eq!(selected.mode, UninstallMode::Interactive);
    }

    #[test]
    fn installed_app_cleanup_is_rejected() {
        assert_eq!(
            ensure_app_removed(true),
            Err("应用仍处于安装状态，不能清理产物")
        );
        assert!(ensure_app_removed(false).is_ok());
    }

    #[test]
    fn missing_uninstaller_and_install_dir_is_stale() {
        let dir = std::env::temp_dir().join(format!("wintop-stale-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let missing_exe = dir.join("missing-uninstall.exe");
        let mut app = test_app(&format!("\"{}\"", missing_exe.to_string_lossy()), "", false, "");
        app.install_location = dir.join("install").to_string_lossy().to_string();

        let capability = capability_for(&app);
        assert_eq!(capability.mode, UninstallMode::Unavailable);
        assert!(capability.stale_entry);
        assert!(capability.message.contains("卸载程序不存在"));
    }

    #[test]
    fn existing_uninstaller_is_interactive() {
        let dir = std::env::temp_dir().join(format!("wintop-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("uninstall.exe");
        fs::write(&exe, b"stub").unwrap();
        let mut app = test_app(&format!("\"{}\"", exe.to_string_lossy()), "", false, "");
        app.install_location = dir.to_string_lossy().to_string();

        let capability = capability_for(&app);
        assert_eq!(capability.mode, UninstallMode::Interactive);
        assert!(!capability.stale_entry);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bare_system_uninstaller_is_available() {
        assert!(command_exe_exists("rundll32.exe"));
        let app = test_app("rundll32.exe \"C:\\x.dll\",Uninstall", "", false, "");
        assert_eq!(capability_for(&app).mode, UninstallMode::Interactive);
    }

    #[test]
    fn no_command_without_install_evidence_is_not_stale() {
        let app = test_app("", "", false, "");
        let capability = capability_for(&app);
        assert_eq!(capability.mode, UninstallMode::Unavailable);
        assert!(!capability.stale_entry);
    }

    #[test]
    fn manifest_without_view_stays_compatible() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"app_id":"a","app_name":"A","items":[{"kind":"reg","original":"HKCU\\X","stored":"C:\\x.reg"}]}"#,
        )
        .unwrap();
        assert!(manifest.items[0].view.is_none());
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
