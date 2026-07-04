//! Windows 视觉效果管理：预设切换（最佳性能 / 最佳外观）+ 快照还原。
//! 通过注册表 HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\VisualEffects 实现。
//! 修改前自动保存原始值到内存，支持一键还原。

use std::sync::Mutex;
use std::sync::LazyLock;
use serde::Serialize;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE,
};

const VISUAL_FX_PATH: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VisualEffects";

static SNAPSHOT: LazyLock<Mutex<Option<u32>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Serialize, Clone)]
pub struct VisualFxState {
    pub mode: String,       // "最佳外观" | "最佳性能" | "自定义"
    pub mode_raw: u32,      // 1=appearance, 2=performance, 3=custom
    pub has_snapshot: bool, // 是否有可还原的快照
}

pub fn get_current_state() -> Result<VisualFxState, String> {
    let raw = read_dword(VISUAL_FX_PATH, "VisualFXSetting").unwrap_or(1);
    let mode = match raw {
        1 => "最佳外观",
        2 => "最佳性能",
        _ => "自定义",
    };
    let has_snapshot = SNAPSHOT.lock().map(|s| s.is_some()).unwrap_or(false);
    Ok(VisualFxState { mode: mode.into(), mode_raw: raw, has_snapshot })
}

/// 应用预设："performance"=最佳性能, "appearance"=最佳外观
pub fn apply_preset(preset: &str) -> Result<String, String> {
    // 首次修改前保存原始值
    if let Ok(mut snap) = SNAPSHOT.lock() {
        if snap.is_none() {
            if let Ok(state) = get_current_state() {
                *snap = Some(state.mode_raw);
            }
        }
    }

    let value: u32 = match preset {
        "performance" => 2,
        "appearance" => 1,
        _ => return Err(format!("未知预设: {}", preset)),
    };

    write_dword(VISUAL_FX_PATH, "VisualFXSetting", value)?;

    // 通知系统刷新（广播 WM_SETTINGCHANGE）
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG,
        };
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            None,
            None,
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }

    let label = match preset {
        "performance" => "最佳性能",
        "appearance" => "最佳外观",
        _ => preset,
    };
    Ok(format!("已切换到「{}」模式", label))
}

/// 还原到修改前的原始值。
pub fn restore_defaults() -> Result<String, String> {
    let original = {
        let mut snap = SNAPSHOT.lock().map_err(|_| "锁异常".to_string())?;
        let val = snap.take();
        val
    };
    match original {
        Some(val) => {
            write_dword(VISUAL_FX_PATH, "VisualFXSetting", val)?;
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    SendMessageTimeoutW, HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG,
                };
                let _ = SendMessageTimeoutW(
                    HWND_BROADCAST,
                    WM_SETTINGCHANGE,
                    None,
                    None,
                    SMTO_ABORTIFHUNG,
                    2000,
                    None,
                );
            }
            let mode = match val { 1 => "最佳外观", 2 => "最佳性能", _ => "自定义" };
            Ok(format!("已还原到「{}」", mode))
        }
        None => Err("没有可还原的快照".into()),
    }
}

// ─── 注册表辅助 ───

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_dword(path: &str, name: &str) -> Option<u32> {
    unsafe {
        let wpath = to_wide(path);
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wpath.as_ptr()), 0, KEY_READ, &mut hkey)
            != ERROR_SUCCESS
        {
            return None;
        }
        let wname = to_wide(name);
        let mut data: u32 = 0;
        let mut len: u32 = 4;
        let r = RegQueryValueExW(
            hkey,
            PCWSTR(wname.as_ptr()),
            None,
            None,
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut len),
        );
        let _ = RegCloseKey(hkey);
        if r == ERROR_SUCCESS { Some(data) } else { None }
    }
}

fn write_dword(path: &str, name: &str, value: u32) -> Result<(), String> {
    unsafe {
        let wpath = to_wide(path);
        let mut hkey = HKEY::default();
        let r = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wpath.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );
        if r != ERROR_SUCCESS {
            return Err(format!("打开注册表项失败: {}", r.0));
        }
        let wname = to_wide(name);
        let bytes = &value.to_le_bytes();
        let r2 = RegSetValueExW(
            hkey,
            PCWSTR(wname.as_ptr()),
            0,
            REG_DWORD,
            Some(bytes),
        );
        let _ = RegCloseKey(hkey);
        if r2 != ERROR_SUCCESS {
            Err(format!("写入注册表失败: {}", r2.0))
        } else {
            Ok(())
        }
    }
}
