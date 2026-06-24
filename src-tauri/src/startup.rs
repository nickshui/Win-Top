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
