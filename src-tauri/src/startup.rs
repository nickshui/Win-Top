//! 启动项：对齐任务管理器「启动应用」的三类来源——
//! 1) 注册表 Run（HKCU / HKLM / HKLM 32 位 WOW6432Node）；
//! 2) 启动文件夹（用户 / 公共，按 Known Folder 解析，支持重定向）；
//! 3) 打包（商店）应用启动任务（AppModel\SystemAppData 下的 State 值）。
//! 启用/禁用写 StartupApproved（Run/Run32/StartupFolder）或 State（UWP），可逆，不删原项。

use std::path::PathBuf;

use serde::Serialize;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

pub use crate::process::ActionResult;

#[derive(Serialize)]
pub struct StartupItem {
    pub id: String,
    pub name: String, // 展示名：Run 值名 / 文件名 / 打包应用友好名
    pub command: String,
    pub location: String, // HKCU-Run | HKLM-Run | HKLM-Run32 | User-Folder | Common-Folder | UWP-Startup
    pub enabled: bool,
}

const RUN_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN32_PATH: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED_RUN: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
const APPROVED_RUN32: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run32";
const APPROVED_FOLDER: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\StartupFolder";
const APP_MODEL_DATA: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData";

/// 纯函数：StartupApproved 二进制值判启用。首字节为奇数即禁用（0x03、0x07…），
/// 偶数即启用（0x02、0x06…）；缺值视为启用。
fn parse_approved_state(bytes: &[u8]) -> bool {
    match bytes.first() {
        Some(b) => b & 1 == 0,
        None => true,
    }
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

/// 读某 hive 下指定键的所有字符串值 (name, command)。
unsafe fn read_reg_values(hive: HKEY, path: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let wpath = to_wide(path);
    let mut hkey = HKEY::default();
    if RegOpenKeyExW(hive, PCWSTR(wpath.as_ptr()), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
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

/// 读某键下所有子键名。
unsafe fn read_subkeys(hive: HKEY, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let wpath = to_wide(path);
    let mut hkey = HKEY::default();
    if RegOpenKeyExW(hive, PCWSTR(wpath.as_ptr()), 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
        return out;
    }
    let mut index = 0u32;
    loop {
        let mut name_buf = [0u16; 512];
        let mut name_len = name_buf.len() as u32;
        let r = RegEnumKeyExW(
            hkey,
            index,
            PWSTR(name_buf.as_mut_ptr()),
            &mut name_len,
            None,
            PWSTR::null(),
            None,
            None,
        );
        if r == ERROR_NO_MORE_ITEMS || r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name_buf[..name_len as usize]));
        index += 1;
    }
    let _ = RegCloseKey(hkey);
    out
}

/// 已打开键上读 DWORD 值。
unsafe fn read_dword_handle(hkey: HKEY, value_name: &str) -> Option<u32> {
    let vname = to_wide(value_name);
    let mut data = [0u8; 4];
    let mut len = data.len() as u32;
    let mut ty = REG_VALUE_TYPE::default();
    let r = RegQueryValueExW(
        hkey,
        PCWSTR(vname.as_ptr()),
        None,
        Some(&mut ty),
        Some(data.as_mut_ptr()),
        Some(&mut len),
    );
    if r == ERROR_SUCCESS && ty == REG_DWORD && len >= 4 {
        Some(u32::from_le_bytes(data))
    } else {
        None
    }
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

/// 启动文件夹路径：优先 Known Folder API（尊重系统重定向），失败退回环境变量。
fn known_folder(rfid: &windows::core::GUID, env_key: &str, tail: &str) -> Option<PathBuf> {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(rfid, KF_FLAG_DEFAULT, HANDLE::default()) {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
            if let Some(s) = s {
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }
    std::env::var_os(env_key).map(|b| PathBuf::from(b).join(tail))
}

// ---------------------------------------------------------------------------
// 打包（商店）应用启动任务
// ---------------------------------------------------------------------------

struct RawUwpTask {
    pfn: String,
    task: String,
    state: u32,
}

/// 枚举 HKCU AppModel\SystemAppData\<PFN>\<Task>，带 State（DWORD）的子键即启动任务。
unsafe fn read_uwp_tasks() -> Vec<RawUwpTask> {
    let mut out = Vec::new();
    for pfn in read_subkeys(HKEY_CURRENT_USER, APP_MODEL_DATA) {
        let pfn_path = format!("{}\\{}", APP_MODEL_DATA, pfn);
        for task in read_subkeys(HKEY_CURRENT_USER, &pfn_path) {
            let task_path = format!("{}\\{}", pfn_path, task);
            let wpath = to_wide(&task_path);
            let mut hkey = HKEY::default();
            if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wpath.as_ptr()), 0, KEY_READ, &mut hkey)
                != ERROR_SUCCESS
            {
                continue;
            }
            let state = read_dword_handle(hkey, "State");
            let _ = RegCloseKey(hkey);
            if let Some(state) = state {
                out.push(RawUwpTask { pfn: pfn.clone(), task, state });
            }
        }
    }
    out
}

/// 纯函数：UWP 启动任务 State → 启用态（0 禁用 / 1 用户禁用 / 2 启用 / 3 策略禁用 / 4 策略启用）。
fn uwp_state_enabled(state: u32) -> bool {
    state == 2 || state == 4
}

/// 纯函数：State 是否由策略管理（不允许用户切换）。
fn uwp_policy_locked(state: u32) -> bool {
    state == 3 || state == 4
}

/// 打包应用友好名（如 "ChatGPT"、"电脑管家"）。通过 shell:AppsFolder 枚举取本地化显示名。
/// 返回 PFN → 显示名映射（只含开始菜单里能解析到的应用）。
fn resolve_uwp_display_names() -> std::collections::HashMap<String, String> {
    std::thread::spawn(move || {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        use windows::Win32::UI::Shell::{
            IEnumShellItems, IShellItem, SHCreateItemFromParsingName, BHID_EnumItems,
            SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
        };
        let mut map = std::collections::HashMap::new();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            // shell:AppsFolder 枚举出的显示名 = 开始菜单/任务管理器里看到的本地化名称。
            // 注意：SHGetKnownFolderPath(FOLDERID_AppsFolder) 对该虚拟文件夹返回 E_FAIL，
            // 必须用字面量 "shell:AppsFolder"。WinRT PackageManager.FindPackages 对
            // 非打包桌面进程返回 E_ACCESSDENIED，不可用。
            let folder_path = to_wide("shell:AppsFolder");
            let folder: Result<IShellItem, _> = SHCreateItemFromParsingName(
                PCWSTR(folder_path.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
            );
            let iter: IEnumShellItems = match folder.and_then(|f| f.BindToHandler(None, &BHID_EnumItems)) {
                Ok(i) => i,
                Err(_) => return map,
            };
            loop {
                let mut items: [Option<IShellItem>; 1] = [None];
                let mut fetched = 0u32;
                if iter.Next(&mut items, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(item) = items[0].take() else { break };
                let name = item
                    .GetDisplayName(SIGDN_NORMALDISPLAY)
                    .map(|p| take_pwstr(p))
                    .unwrap_or_default();
                let parsing = item
                    .GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)
                    .map(|p| take_pwstr(p))
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                // 解析名形如 "::{4234d49b-...}\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"
                let aumid = parsing.rsplit('\\').next().unwrap_or("");
                if let Some((pfn, _)) = aumid.split_once('!') {
                    map.entry(pfn.to_string()).or_insert(name);
                }
            }
        }
        map
    })
    .join()
    .unwrap_or_default()
}

/// 取 PWSTR 并释放（Shell API 返回的字符串由 CoTaskMemAlloc 分配）。
unsafe fn take_pwstr(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
    s
}

fn set_uwp_enabled(pfn: &str, task: &str, enabled: bool) -> Result<(), String> {
    let task_path = format!("{}\\{}\\{}", APP_MODEL_DATA, pfn, task);
    unsafe {
        let wpath = to_wide(&task_path);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wpath.as_ptr()),
            0,
            KEY_READ | KEY_SET_VALUE,
            &mut hkey,
        ) != ERROR_SUCCESS
        {
            return Err("打开启动任务注册表项失败".into());
        }
        if let Some(state) = read_dword_handle(hkey, "State") {
            if uwp_policy_locked(state) {
                let _ = RegCloseKey(hkey);
                return Err("该启动任务由系统策略管理，无法在此切换".into());
            }
        }
        let value = if enabled { 2u32 } else { 1u32 };
        let bytes = value.to_le_bytes();
        let vname = to_wide("State");
        let r = RegSetValueExW(hkey, PCWSTR(vname.as_ptr()), 0, REG_DWORD, Some(&bytes));
        let _ = RegCloseKey(hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("写入失败: {}", r.0));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 列表 / 切换
// ---------------------------------------------------------------------------

pub fn list_startup() -> Vec<StartupItem> {
    let mut items = Vec::new();
    unsafe {
        // 1) 注册表 Run（HKCU / HKLM / HKLM 32 位）
        let runs: [(HKEY, &str, &str, HKEY, &str); 3] = [
            (HKEY_CURRENT_USER, RUN_PATH, "HKCU-Run", HKEY_CURRENT_USER, APPROVED_RUN),
            (HKEY_LOCAL_MACHINE, RUN_PATH, "HKLM-Run", HKEY_LOCAL_MACHINE, APPROVED_RUN),
            (HKEY_LOCAL_MACHINE, RUN32_PATH, "HKLM-Run32", HKEY_LOCAL_MACHINE, APPROVED_RUN32),
        ];
        for (run_hive, run_path, loc, appr_hive, appr_path) in runs {
            for (name, cmd) in read_reg_values(run_hive, run_path) {
                let enabled = approved_state(appr_hive, appr_path, &name);
                items.push(StartupItem {
                    id: format!("{}|{}", loc, name),
                    name: name.clone(),
                    command: cmd,
                    location: loc.into(),
                    enabled,
                });
            }
        }

        // 2) 启动文件夹（用户 / 公共）；任务管理器的禁用记录统一写在 HKCU StartupApproved\StartupFolder
        let folders = [
            (
                "User-Folder",
                known_folder(
                    &FOLDERID_Startup,
                    "APPDATA",
                    "Microsoft\\Windows\\Start Menu\\Programs\\Startup",
                ),
            ),
            (
                "Common-Folder",
                known_folder(
                    &FOLDERID_CommonStartup,
                    "ProgramData",
                    "Microsoft\\Windows\\Start Menu\\Programs\\Startup",
                ),
            ),
        ];
        for (loc, dir) in folders {
            if let Some(dir) = dir {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let fname = e.file_name().to_string_lossy().to_string();
                        if fname.eq_ignore_ascii_case("desktop.ini") {
                            continue;
                        }
                        let enabled = approved_state(HKEY_CURRENT_USER, APPROVED_FOLDER, &fname);
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

        // 3) 打包应用启动任务
        let raw = read_uwp_tasks();
        let names = if raw.is_empty() {
            std::collections::HashMap::new()
        } else {
            resolve_uwp_display_names()
        };
        for r in raw {
            let display = names.get(&r.pfn).cloned().unwrap_or_else(|| r.pfn.clone());
            items.push(StartupItem {
                id: format!("UWP|{}|{}", r.pfn, r.task),
                name: display,
                command: r.pfn,
                location: "UWP-Startup".into(),
                enabled: uwp_state_enabled(r.state),
            });
        }
    }

    // 与任务管理器一致：启用的排前面，其次按名称不区分大小写排序
    items.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    items
}

pub fn set_startup_enabled(id: String, enabled: bool) -> ActionResult {
    if let Some(rest) = id.strip_prefix("UWP|") {
        let mut parts = rest.splitn(2, '|');
        let (pfn, task) = match (parts.next(), parts.next()) {
            (Some(p), Some(t)) => (p, t),
            _ => {
                return ActionResult {
                    success: false,
                    message: "无效的启动项 id".into(),
                }
            }
        };
        return match set_uwp_enabled(pfn, task, enabled) {
            Ok(_) => ActionResult {
                success: true,
                message: format!("已{}该应用启动任务", if enabled { "启用" } else { "禁用" }),
            },
            Err(e) => ActionResult {
                success: false,
                message: format!("操作失败：{}", e),
            },
        };
    }

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
        "HKLM-Run32" => (HKEY_LOCAL_MACHINE, APPROVED_RUN32),
        // 启动文件夹项的禁用记录是每用户的，统一写 HKCU（公共文件夹项也一样）
        "User-Folder" | "Common-Folder" => (HKEY_CURRENT_USER, APPROVED_FOLDER),
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
    use super::{encode_approved, parse_approved_state, uwp_policy_locked, uwp_state_enabled};

    #[test]
    fn approved_state_parsing() {
        assert!(parse_approved_state(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse_approved_state(&[0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!parse_approved_state(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        // 0x07 = 重新禁用（带时间戳），按最小值位判定为禁用
        assert!(!parse_approved_state(&[0x07, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(parse_approved_state(&[])); // 缺值视为启用
    }

    #[test]
    fn encode_roundtrip() {
        assert_eq!(encode_approved(true)[0], 0x02);
        assert_eq!(encode_approved(false)[0], 0x03);
        assert_eq!(encode_approved(true).len(), 12);
    }

    #[test]
    fn uwp_state_mapping() {
        assert!(!uwp_state_enabled(0));
        assert!(!uwp_state_enabled(1));
        assert!(uwp_state_enabled(2));
        assert!(!uwp_state_enabled(3));
        assert!(uwp_state_enabled(4));
        assert!(uwp_policy_locked(3));
        assert!(uwp_policy_locked(4));
        assert!(!uwp_policy_locked(2));
    }

    /// 真机冒烟：列表非空、id 唯一；启用项排在最前。
    #[test]
    fn list_startup_smoke() {
        let items = super::list_startup();
        assert!(!items.is_empty(), "真机应能读到至少一个启动项");
        let ids: std::collections::HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), items.len(), "启动项 id 必须唯一");
        let first_disabled = items.iter().position(|i| !i.enabled);
        if let Some(pos) = first_disabled {
            assert!(
                items[pos..].iter().all(|i| !i.enabled),
                "启用项必须排在禁用项之前"
            );
        }
        for it in &items {
            println!("{} | {} | enabled={} | {}", it.location, it.name, it.enabled, it.command);
        }
    }
}

