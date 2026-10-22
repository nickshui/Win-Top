//! 更新检查与自动安装：查询 GitHub Releases 最新正式版本，并支持应用内自动更新。
//!
//! # 与 Tauri 内置 updater 的区别
//!
//! Tauri v1 / v2 的官方 updater 都**强制要求 minisign 签名密钥**（v2 的 `Config.pubkey`
//! 也是必填 `String`），且需要额外产出 `.sig` / `latest.json`。本项目选择不引入密钥与 CI，
//! 改为自己实现一条等价链路：
//!
//! 1. 用 GitHub Releases API 读取最新正式版本与安装包直链；
//! 2. 下载安装包时用 GitHub 资产自带的 `digest`（sha256）校验完整性；
//! 3. 按当前安装形态分三路安装：
//!    * **便携版**：下载新版 exe → 启动自身作为助手进程 → 助手等旧进程退出后原地替换 exe →
//!      启动新版；新实例启动时清理 `.old` 备份；
//!    * **NSIS 安装版**：下载 setup.exe → `ShellExecuteW` 以 `/P /R /NS /UPDATE` 运行
//!      （与 Tauri v1 updater 的 passive 模式完全一致），安装完成后自动重启；
//!    * **MSI 安装版**：下载 .msi → `msiexec /i ... /passive /promptrestart AUTOLAUNCHAPP=True`，
//!      由 shell 触发 UAC。
//!
//! 安全边界：只接受 `https://github.com/nickshui/Win-Top/releases/download/` 前缀的地址；
//! 校验文件大小、魔数与 sha256 摘要；不做静默提权。仍未引入代码签名/minisign ——
//! 若仓库账号被接管，SHA-256 摘要同样会被篡改，这一点与"手动去 Releases 下载"等价；
//! 需要更强保证时应改用带签名密钥的方案。
//!
//! # 数据来源
//!
//! GitHub 官方 REST API `repos/{owner}/{repo}/releases/latest`：只返回最新**正式** release
//! （排除 draft / prerelease），无需 token；未认证时按 IP 限流 60 次/小时。

use std::fs;
use std::io::{Read, Write};
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, CloseHandle, ERROR_SUCCESS, HWND};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS,
};
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MB_ICONERROR, MB_OK, SW_SHOWNORMAL,
};

use crate::app_inventory;

/// 发行仓库（与 README、发版流程保持一致）。
const RELEASES_PAGE: &str = "https://github.com/nickshui/Win-Top/releases";
const API_LATEST: &str = "https://api.github.com/repos/nickshui/Win-Top/releases/latest";
/// 只允许从官方 Release 资产地址下载，防止被诱导执行任意 URL。
const DOWNLOAD_PREFIX: &str = "https://github.com/nickshui/Win-Top/releases/download/";
/// 产品名（与 tauri.conf.json 的 productName 一致，用于匹配卸载注册表项）。
const PRODUCT_NAME: &str = "Win-Top";
/// 便携版标记：与 exe 同级存在即视为便携版（与 history_store 的约定一致）。
const PORTABLE_MARKER: &str = "wintop.portable";
/// 更新助手命令行标志（便携版原地替换用）。
const HELPER_FLAG: &str = "--apply-portable-update";
/// 网络超时：检查更新是用户可感知的操作，不能长时间挂起。
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// 下载大文件时的读超时（比检查宽松）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// 对外数据结构
// ---------------------------------------------------------------------------

/// 一个可下载的发行产物。
#[derive(Serialize, Clone)]
pub struct UpdateAsset {
    /// 资产文件名，如 `Win-Top_0.1.7_x64-setup.exe`。
    pub name: String,
    /// 字节数。
    pub size: u64,
    /// 分类：`nsis` / `msi` / `portable` / `other`。
    pub kind: String,
    /// 浏览器直链（GitHub 会 302 到 objects.githubusercontent.com）。
    pub url: String,
    /// GitHub 提供的 sha256 摘要（形如 `sha256:...`），老版本可能为空。
    pub digest: Option<String>,
}

/// 一次版本检查的结果。
#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub title: String,
    pub notes: String,
    pub published_at: String,
    pub html_url: String,
    pub assets: Vec<UpdateAsset>,
    /// 当前安装形态：`portable` / `nsis` / `msi`。
    pub edition: String,
    /// 安装形态的中文名。
    pub edition_label: String,
    /// 是否支持「应用内自动安装」（便携版需要 exe 所在目录可写）。
    pub can_self_install: bool,
}

/// 当前程序的安装形态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edition {
    Portable,
    Nsis,
    Msi,
}

impl Edition {
    fn as_str(self) -> &'static str {
        match self {
            Edition::Portable => "portable",
            Edition::Nsis => "nsis",
            Edition::Msi => "msi",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Edition::Portable => "便携版",
            Edition::Nsis => "NSIS 安装版",
            Edition::Msi => "MSI 安装版",
        }
    }

    /// 当前形态对应的资产分类（见 [`asset_kind`]）。
    fn asset_kind(self) -> &'static str {
        match self {
            Edition::Portable => "portable",
            Edition::Nsis => "nsis",
            Edition::Msi => "msi",
        }
    }
}

#[derive(Deserialize)]
struct GhRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    browser_download_url: String,
    /// GitHub 自 2025 年起为资产提供 sha256 摘要；老仓库/旧资产可能没有。
    #[serde(default)]
    digest: Option<String>,
}

// ---------------------------------------------------------------------------
// 版本检查
// ---------------------------------------------------------------------------

/// 查询 GitHub 最新正式 release，并与 `current_version` 比较。
pub fn check_update(current_version: &str) -> Result<UpdateInfo, String> {
    let client = reqwest::blocking::Client::builder()
        // GitHub API 强制要求 User-Agent，缺省会被 403。
        .user_agent(format!("Win-Top/{}", normalize_version(current_version)))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("初始化网络请求失败：{}", e))?;

    let resp = client
        .get(API_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("无法连接 GitHub：{}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            403 | 429 => "（GitHub API 限流：未登录每小时 60 次，请稍后再试）",
            404 => "（仓库或 release 不存在）",
            _ => "",
        };
        return Err(format!("GitHub 返回 HTTP {}{}", status.as_u16(), hint));
    }

    let release: GhRelease = resp
        .json()
        .map_err(|e| format!("解析 GitHub 返回失败：{}", e))?;

    let latest = normalize_version(&release.tag_name);
    if latest.is_empty() {
        return Err("GitHub Release 缺少 tag，无法比较版本".to_string());
    }

    let mut assets: Vec<UpdateAsset> = release
        .assets
        .into_iter()
        .filter(|a| is_installer_asset(&a.name) && !a.browser_download_url.trim().is_empty())
        .map(|a| {
            let kind = asset_kind(&a.name).to_string();
            UpdateAsset {
                name: a.name,
                size: a.size,
                kind,
                url: a.browser_download_url,
                digest: a.digest,
            }
        })
        .collect();
    // 稳定排序：NSIS 安装包排最前（与 README 的推荐一致）。
    assets.sort_by_key(|a| asset_rank(&a.kind));

    let edition = detect_edition();

    Ok(UpdateInfo {
        current_version: normalize_version(current_version),
        latest_version: latest.clone(),
        has_update: is_newer(&latest, current_version),
        title: release
            .name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("Win-Top v{}", latest)),
        notes: release.body.unwrap_or_default(),
        published_at: release.published_at.unwrap_or_default(),
        html_url: release
            .html_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| RELEASES_PAGE.to_string()),
        assets,
        edition: edition.as_str().to_string(),
        edition_label: edition.label().to_string(),
        can_self_install: match edition {
            Edition::Portable => current_exe_dir().map(|d| dir_writable(&d)).unwrap_or(false),
            _ => true,
        },
    })
}

// ---------------------------------------------------------------------------
// 版本 / 资产工具
// ---------------------------------------------------------------------------

/// 去掉首尾空白与一个前导 `v` / `V`（`v0.1.7` → `0.1.7`）。
fn normalize_version(v: &str) -> String {
    let t = v.trim();
    let t = t
        .strip_prefix('v')
        .or_else(|| t.strip_prefix('V'))
        .unwrap_or(t);
    t.trim().to_string()
}

/// 只保留形似安装包的资产，避免把源码包/说明文件列进下载列表。
fn is_installer_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".exe") || lower.ends_with(".msi")
}

/// 资产分类（顺序即前端展示文案的依据）。
fn asset_kind(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("portable") {
        "portable"
    } else if lower.ends_with(".msi") {
        "msi"
    } else if lower.ends_with("-setup.exe") || lower.ends_with("_setup.exe") {
        "nsis"
    } else {
        "other"
    }
}

fn asset_rank(kind: &str) -> u8 {
    match kind {
        "nsis" => 0,
        "msi" => 1,
        "portable" => 2,
        _ => 3,
    }
}

/// 解析成 `(major, minor, patch)`；允许缺省 minor/patch，遇到非数字返回 None。
fn version_core(v: &str) -> Option<(u64, u64, u64)> {
    let normalized = normalize_version(v);
    let core = normalized
        .split(|c| c == '-' || c == '+')
        .next()
        .unwrap_or(&normalized);
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// `latest` 是否严格新于 `current`；任一侧解析失败都返回 false。
fn is_newer(latest: &str, current: &str) -> bool {
    match (version_core(latest), version_core(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 安装形态检测
// ---------------------------------------------------------------------------

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// 检测当前运行形态：便携版（标记文件 / 未在卸载注册表登记）→ 便携版；
/// 否则按 WindowsInstaller 标志区分 MSI 与 NSIS。
fn detect_edition() -> Edition {
    let Some(dir) = current_exe_dir() else {
        return Edition::Portable;
    };
    if dir.join(PORTABLE_MARKER).exists() {
        return Edition::Portable;
    }
    match installed_edition(&dir) {
        Some(true) => Edition::Msi,
        Some(false) => Edition::Nsis,
        None => Edition::Portable,
    }
}

/// 在卸载注册表三处里找与本程序目录匹配的条目，返回 `Some(是否 MSI)`。
fn installed_edition(exe_dir: &Path) -> Option<bool> {
    let target = normalize_dir(exe_dir)?;
    let path = app_inventory::to_wide("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
    let roots: [(HKEY, REG_SAM_FLAGS); 3] = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY),
        (HKEY_CURRENT_USER, KEY_READ),
    ];

    for (hive, sam) in roots {
        unsafe {
            let mut root = HKEY::default();
            if RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), 0, sam, &mut root) != ERROR_SUCCESS {
                continue;
            }
            for subname in app_inventory::enum_subkey_names(root) {
                let subw = app_inventory::to_wide(&subname);
                let mut sub = HKEY::default();
                if RegOpenKeyExW(root, PCWSTR(subw.as_ptr()), 0, sam, &mut sub) != ERROR_SUCCESS {
                    continue;
                }
                let name = app_inventory::read_string(sub, "DisplayName");
                if name.eq_ignore_ascii_case(PRODUCT_NAME) {
                    let is_msi =
                        app_inventory::read_u32(sub, "WindowsInstaller").unwrap_or(0) == 1;
                    let matched = ["InstallLocation", "DisplayIcon", "UninstallString"]
                        .iter()
                        .any(|value_name| {
                            let raw = app_inventory::read_string(sub, value_name);
                            value_to_dir(&raw)
                                .as_deref()
                                .and_then(normalize_dir)
                                .map(|d| d == target)
                                .unwrap_or(false)
                        });
                    let _ = RegCloseKey(sub);
                    if matched {
                        let _ = RegCloseKey(root);
                        return Some(is_msi);
                    }
                    continue;
                }
                let _ = RegCloseKey(sub);
            }
            let _ = RegCloseKey(root);
        }
    }
    None
}

/// 把注册表值解释成"程序所在目录"：
/// * `InstallLocation` → 目录本身；
/// * `DisplayIcon`（`C:\...\Win-Top.exe,0`）→ 去掉 `,N` 后取父目录；
/// * `UninstallString`（`"C:\...\uninstall.exe"`）→ 去引号后取父目录。
fn value_to_dir(raw: &str) -> Option<PathBuf> {
    let v = raw.trim().trim_matches('"').trim();
    if v.is_empty() {
        return None;
    }
    // DisplayIcon 可能形如 "...\app.exe,0"；路径本身不会含逗号后的纯数字段。
    let v = v.split(',').next().unwrap_or(v).trim();
    let v = v.trim_matches('"').trim();
    let p = Path::new(v);
    if p.is_dir() {
        Some(p.to_path_buf())
    } else {
        p.parent().map(Path::to_path_buf)
    }
}

/// 目录路径归一化：尽量 canonicalize，再统一小写并去尾部斜杠。
fn normalize_dir(p: &Path) -> Option<String> {
    let canon = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy().to_lowercase();
    let s = s.trim_end_matches(['\\', '/']).to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// exe 所在目录是否可写（便携版原地替换的前提）。
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".wintop-write-test-{}", std::process::id()));
    match fs::write(&probe, b"x") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// 下载与安装
// ---------------------------------------------------------------------------

fn emit_progress(app: &tauri::AppHandle, phase: &str, downloaded: u64, total: u64) {
    let pct = if total > 0 {
        (downloaded as f64 / total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };
    let _ = app.emit_all(
        "update-progress",
        serde_json::json!({
            "phase": phase,
            "downloaded": downloaded,
            "total": total,
            "pct": pct,
        }),
    );
}

/// 下载更新包 → 校验大小/sha256/魔数 → 按安装形态安装 → 退出当前进程。
///
/// 成功路径**不会返回**（进程会被替换/退出），失败返回人话错误供前端提示。
pub fn install_update(
    app: &tauri::AppHandle,
    url: &str,
    kind: &str,
    digest: Option<&str>,
    size: u64,
) -> Result<(), String> {
    if !url.starts_with(DOWNLOAD_PREFIX) {
        return Err("更新地址不是官方 Release 地址，已拒绝。".to_string());
    }

    let edition = detect_edition();
    if edition.asset_kind() != kind {
        return Err(format!(
            "更新包类型（{}）与当前安装方式（{}）不匹配，请改用浏览器手动下载。",
            kind,
            edition.label()
        ));
    }

    let dest = temp_installer_path(url);
    emit_progress(app, "downloading", 0, size);
    let sha = download_with_progress(app, url, &dest, size)?;
    emit_progress(app, "verifying", size, size);

    if size > 0 {
        let actual = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if actual != size {
            let _ = fs::remove_file(&dest);
            return Err(format!(
                "下载不完整（期望 {} 字节，实际 {} 字节），已取消安装。",
                size, actual
            ));
        }
    }
    if let Some(expected) = digest {
        let want = expected
            .trim()
            .strip_prefix("sha256:")
            .unwrap_or(expected.trim())
            .to_ascii_lowercase();
        if !want.is_empty() && sha != want {
            let _ = fs::remove_file(&dest);
            return Err("更新包 SHA-256 校验失败，已取消安装。".to_string());
        }
    }
    // 粗粒度格式校验：防止把 HTML 错误页当安装包执行。
    let magic_ok = match kind {
        "msi" => starts_with_bytes(&dest, &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
        _ => starts_with_bytes(&dest, b"MZ"),
    };
    if !magic_ok {
        let _ = fs::remove_file(&dest);
        return Err("下载内容不是可执行安装包，已取消安装。".to_string());
    }

    emit_progress(app, "installing", size, size);

    match edition {
        Edition::Portable => spawn_portable_helper(&dest)?,
        Edition::Nsis => shell_execute_path(&dest, "/P /R /NS /UPDATE")?,
        Edition::Msi => {
            let msiexec = msiexec_path();
            let params = format!(
                "/i \"{}\" /passive /promptrestart AUTOLAUNCHAPP=True",
                dest.display()
            );
            shell_execute_path(Path::new(&msiexec), &params)?;
        }
    }

    // 让历史写线程把最后一批数据落盘，然后退出，交给安装器/助手继续。
    crate::history_store::shutdown();
    std::process::exit(0);
}

fn temp_installer_path(url: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("WinTop-update");
    let _ = fs::create_dir_all(&dir);
    let raw = url.rsplit('/').next().unwrap_or("installer.bin");
    let name: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    dir.join(name)
}

/// 流式下载并计算 sha256；每 ~100ms 向界面推一次进度。
fn download_with_progress(
    app: &tauri::AppHandle,
    url: &str,
    dest: &Path,
    expected_size: u64,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Win-Top-updater/1")
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| format!("初始化下载失败：{}", e))?;
    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| format!("下载更新包失败：{}", e))?;
    if !resp.status().is_success() {
        return Err(format!(
            "下载更新包失败：HTTP {}",
            resp.status().as_u16()
        ));
    }

    let total = resp.content_length().unwrap_or(expected_size);
    let mut file = fs::File::create(dest).map_err(|e| format!("创建临时文件失败：{}", e))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("下载中断：{}", e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("写入临时文件失败：{}", e))?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(100) {
            last_emit = Instant::now();
            emit_progress(app, "downloading", downloaded, total);
        }
    }
    let _ = file.flush();
    let _ = file.sync_all();
    emit_progress(app, "downloading", downloaded, total);

    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

fn starts_with_bytes(path: &Path, prefix: &[u8]) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; prefix.len()];
    f.read_exact(&mut buf).is_ok() && buf == prefix
}

fn msiexec_path() -> String {
    std::env::var("SYSTEMROOT")
        .map(|root| format!("{}\\System32\\msiexec.exe", root))
        .unwrap_or_else(|_| "msiexec.exe".to_string())
}

/// `ShellExecuteW` 启动安装器；失败返回错误（UAC 取消等）。
fn shell_execute_path(file: &Path, params: &str) -> Result<(), String> {
    let file_w: Vec<u16> = file.as_os_str().encode_wide().chain(once(0)).collect();
    let params_w: Vec<u16> = params.encode_utf16().chain(once(0)).collect();
    let result = unsafe {
        // worker 线程默认未初始化 COM；ShellExecuteW 走 shell/提权路径时需要 COM。
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let h = ShellExecuteW(
            HWND::default(),
            PCWSTR::null(),
            PCWSTR(file_w.as_ptr()),
            PCWSTR(params_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if com.is_ok() {
            CoUninitialize();
        }
        h
    };
    if (result.0 as isize) <= 32 {
        return Err("启动安装程序失败（可能取消了 UAC 或安装器无效）。".to_string());
    }
    Ok(())
}

/// 便携版：启动一个自身副本作为助手，等当前进程退出后原地替换 exe 并重启。
fn spawn_portable_helper(source: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("读取当前程序路径失败：{}", e))?;
    if !dir_writable(exe.parent().unwrap_or(Path::new("."))) {
        return Err("当前程序所在目录不可写，无法自动替换，请在浏览器中下载新版。".to_string());
    }
    Command::new(&exe)
        .arg(HELPER_FLAG)
        .arg(source)
        .arg(&exe)
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|e| format!("启动更新助手失败：{}", e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 便携版更新助手（独立进程，在 main 里先于 Tauri 初始化处理）
// ---------------------------------------------------------------------------

/// 若当前进程是更新助手则执行替换并返回退出码；否则返回 None 继续正常启动。
pub fn run_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args_os();
    let _exe = args.next();
    let flag = args.next()?;
    if flag.to_string_lossy() != HELPER_FLAG {
        return None;
    }
    let source = PathBuf::from(args.next()?);
    let target = PathBuf::from(args.next()?);
    let parent_pid = args
        .next()
        .and_then(|s| s.to_string_lossy().parse::<u32>().ok())
        .unwrap_or(0);
    Some(apply_portable_update(&source, &target, parent_pid))
}

fn apply_portable_update(source: &Path, target: &Path, parent_pid: u32) -> i32 {
    helper_log(&format!(
        "start: source={} target={} parent={}",
        source.display(),
        target.display(),
        parent_pid
    ));

    // 等旧进程完全退出（文件句柄释放）再替换。
    wait_for_process_exit(parent_pid, Duration::from_secs(120));

    let mut backup_os = target.as_os_str().to_os_string();
    backup_os.push(".old");
    let backup = PathBuf::from(&backup_os);
    let _ = fs::remove_file(&backup);

    let mut renamed = false;
    let mut last_err = String::new();
    for _ in 0..40 {
        match fs::rename(target, &backup) {
            Ok(_) => {
                renamed = true;
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if !renamed {
        let msg = format!("无法重命名当前程序（文件仍被占用）：{}", last_err);
        helper_log(&msg);
        fail_with_message("更新失败", &format!("{}\n\n请手动到 Releases 页面下载新版。", msg));
        return 1;
    }

    let mut copied = false;
    for _ in 0..40 {
        match fs::copy(source, target) {
            Ok(_) => {
                copied = true;
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    if !copied {
        // 回滚到旧程序，避免更新失败后无法启动。
        let _ = fs::rename(&backup, target);
        let msg = format!("写入新版程序失败：{}", last_err);
        helper_log(&msg);
        fail_with_message("更新失败", &format!("{}\n\n已回滚到旧版本。", msg));
        return 1;
    }

    let _ = fs::remove_file(source);
    match Command::new(target).spawn() {
        Ok(_) => {
            helper_log("replaced and relaunched");
            0
        }
        Err(e) => {
            let msg = format!("新版程序已就位，但自动启动失败：{}", e);
            helper_log(&msg);
            fail_with_message("更新完成但启动失败", &format!("{}\n\n请手动启动新版。", msg));
            1
        }
    }
}

fn wait_for_process_exit(pid: u32, timeout: Duration) {
    if pid == 0 {
        return;
    }
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, BOOL(0), pid) {
            let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            let _ = WaitForSingleObject(handle, ms);
            let _ = CloseHandle(handle);
        }
    }
}

fn fail_with_message(title: &str, message: &str) {
    let title_w: Vec<u16> = title.encode_utf16().chain(once(0)).collect();
    let msg_w: Vec<u16> = message.encode_utf16().chain(once(0)).collect();
    unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(msg_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn helper_log(msg: &str) {
    let path = std::env::temp_dir().join("WinTop-update.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            f,
            "[{}] {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            msg
        );
    }
}

/// 启动时清理助手替换留下的 `.old` 备份（新进程退出前它一直被占用）。
pub fn cleanup_old_binary() {
    std::thread::spawn(|| {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let mut old_os = exe.as_os_str().to_os_string();
        old_os.push(".old");
        let old = PathBuf::from(old_os);
        if !old.exists() {
            return;
        }
        for _ in 0..40 {
            if fs::remove_file(&old).is_ok() {
                helper_log("cleaned up .old backup");
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_prefix_and_spaces() {
        assert_eq!(normalize_version(" v0.1.7 "), "0.1.7");
        assert_eq!(normalize_version("V1.2.3"), "1.2.3");
        assert_eq!(normalize_version("0.1.7"), "0.1.7");
    }

    #[test]
    fn version_comparison_is_numeric_not_lexicographic() {
        assert!(is_newer("0.1.8", "0.1.7"));
        assert!(is_newer("0.1.10", "0.1.9"), "0.1.10 必须大于 0.1.9");
        assert!(is_newer("v0.2.0", "0.1.99"));
        assert!(!is_newer("0.1.7", "0.1.7"));
        assert!(!is_newer("0.1.7", "0.1.8"));
        assert!(!is_newer("garbage", "0.1.7"));
        assert!(!is_newer("0.1.7", "garbage"));
    }

    #[test]
    fn prerelease_core_is_compared_by_numbers() {
        assert!(is_newer("0.2.0-beta.1", "0.1.7"));
        assert!(!is_newer("0.1.7-beta.1", "0.1.7"));
    }

    #[test]
    fn assets_are_classified_for_download_list() {
        assert_eq!(asset_kind("Win-Top_0.1.7_x64-setup.exe"), "nsis");
        assert_eq!(asset_kind("Win-Top_0.1.7_x64_en-US.msi"), "msi");
        assert_eq!(asset_kind("Win-Top_0.1.7_portable_x64.exe"), "portable");
        assert_eq!(asset_kind("something-else.exe"), "other");
        assert!(is_installer_asset("Win-Top_0.1.7_x64-setup.exe"));
        assert!(is_installer_asset("Win-Top_0.1.7_x64_en-US.msi"));
        assert!(!is_installer_asset("Win-Top_0.1.7_x64-setup.nsis.zip"));
        assert!(!is_installer_asset("latest.json"));
    }

    #[test]
    fn installer_assets_sort_with_nsis_first() {
        let mut kinds = vec!["other", "portable", "msi", "nsis"];
        kinds.sort_by_key(|k| asset_rank(k));
        assert_eq!(kinds, vec!["nsis", "msi", "portable", "other"]);
    }

    #[test]
    fn edition_maps_to_asset_kind() {
        assert_eq!(Edition::Portable.asset_kind(), "portable");
        assert_eq!(Edition::Nsis.asset_kind(), "nsis");
        assert_eq!(Edition::Msi.asset_kind(), "msi");
    }

    #[test]
    fn registry_value_to_dir_handles_common_shapes() {
        // 目录本身
        let dir = std::env::temp_dir();
        assert_eq!(
            value_to_dir(&dir.to_string_lossy()),
            Some(dir.clone())
        );
        // DisplayIcon 带图标索引
        let icon = format!("{}\\app.exe,0", dir.to_string_lossy());
        assert_eq!(value_to_dir(&icon), Some(dir.clone()));
        // UninstallString 带引号
        let uninstall = format!("\"{}\\uninstall.exe\"", dir.to_string_lossy());
        assert_eq!(value_to_dir(&uninstall), Some(dir));
        assert_eq!(value_to_dir("   "), None);
    }

    #[test]
    fn detect_edition_returns_a_known_variant() {
        // 环境相关（本机可能是便携版或安装版），只验证不 panic 且是三种之一。
        let edition = detect_edition();
        assert!(matches!(
            edition,
            Edition::Portable | Edition::Nsis | Edition::Msi
        ));
    }
}
