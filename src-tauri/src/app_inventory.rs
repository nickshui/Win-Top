//! 已装软件清单：枚举注册表三处 Uninstall 键（HKLM 64 位 / HKLM 32 位视图 / HKCU），
//! 过滤系统组件与更新补丁，产出用户可见的应用列表。纯读，无需管理员。
//!
//! 设计说明：MSI 应用同样在 ARP（Uninstall 键）里有条目，故注册表即为权威清单来源；
//! 通过 WindowsInstaller 标志 / 子键名是否为 ProductCode GUID 识别 MSI，供卸载走 msiexec。

use serde::Serialize;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS, REG_VALUE_TYPE,
};

const UNINSTALL_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

#[derive(Serialize, Clone)]
pub struct AppEntry {
    pub id: String, // "HKLM|<subkey>" 等，唯一，供后续解析/卸载重开键
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub install_date: String, // 原始 YYYYMMDD 或空
    pub install_location: String,
    pub estimated_size_kb: u64,
    pub uninstall_string: String,
    pub quiet_uninstall_string: String,
    pub display_icon: String,
    pub is_msi: bool,
    pub product_code: String, // MSI 时为 {GUID}
    pub arch: String,         // x64 | x86 | user
    pub source: String,       // HKLM | HKLM-WOW | HKCU
}

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 子键名是否是 MSI ProductCode 形式 `{8-4-4-4-12}`。
fn is_guid_keyname(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 38 && b[0] == b'{' && b[37] == b'}'
}

/// 展开路径中的 %VAR%（InstallLocation 可能是 REG_EXPAND_SZ）。
pub(crate) fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        if let Some(end_rel) = rest[start + 1..].find('%') {
            let var = &rest[start + 1..start + 1 + end_rel];
            match std::env::var(var) {
                Ok(val) => out.push_str(&val),
                Err(_) => {
                    out.push('%');
                    out.push_str(var);
                    out.push('%');
                }
            }
            rest = &rest[start + 1 + end_rel + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// 读注册表值原始字节 + 类型；缺失/失败返回 None。用两次调用（先取长度）保证不截断。
unsafe fn read_value_raw(hkey: HKEY, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let namew = to_wide(name);
    let mut vtype = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    let r = RegQueryValueExW(
        hkey,
        PCWSTR(namew.as_ptr()),
        None,
        Some(&mut vtype),
        None,
        Some(&mut len),
    );
    if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
        return None;
    }
    if len == 0 {
        return Some((vtype, Vec::new()));
    }
    let mut buf = vec![0u8; len as usize];
    let mut len2 = len;
    let r2 = RegQueryValueExW(
        hkey,
        PCWSTR(namew.as_ptr()),
        None,
        Some(&mut vtype),
        Some(buf.as_mut_ptr()),
        Some(&mut len2),
    );
    if r2 != ERROR_SUCCESS {
        return None;
    }
    buf.truncate(len2 as usize);
    Some((vtype, buf))
}

/// 读字符串值（REG_SZ / REG_EXPAND_SZ），去掉尾部 NUL；缺失返回空串。
pub(crate) unsafe fn read_string(hkey: HKEY, name: &str) -> String {
    match read_value_raw(hkey, name) {
        Some((_t, bytes)) if bytes.len() >= 2 => {
            let u16s: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&u16s)
                .trim_end_matches('\0')
                .trim()
                .to_string()
        }
        _ => String::new(),
    }
}

/// 读 DWORD 值。
unsafe fn read_u32(hkey: HKEY, name: &str) -> Option<u32> {
    match read_value_raw(hkey, name) {
        Some((_t, bytes)) if bytes.len() >= 4 => {
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
        _ => None,
    }
}

/// 枚举一个键下的所有子键名。
pub(crate) unsafe fn enum_subkey_names(hkey: HKEY) -> Vec<String> {
    let mut out = Vec::new();
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
        if r != ERROR_SUCCESS {
            break; // 含 ERROR_NO_MORE_ITEMS
        }
        out.push(String::from_utf16_lossy(&name_buf[..name_len as usize]));
        index += 1;
    }
    out
}

/// 从一个 Uninstall 子键读出 AppEntry；被判为系统组件/更新补丁/无名则返回 None。
unsafe fn read_app(hkey: HKEY, subname: &str, source: &str, arch: &str) -> Option<AppEntry> {
    let name = read_string(hkey, "DisplayName");
    if name.is_empty() {
        return None;
    }
    // 过滤：系统组件、更新补丁（有父项/发布类型为更新）
    if read_u32(hkey, "SystemComponent").unwrap_or(0) == 1 {
        return None;
    }
    if !read_string(hkey, "ParentKeyName").is_empty() {
        return None;
    }
    let rel = read_string(hkey, "ReleaseType");
    if matches!(rel.as_str(), "Security Update" | "Update" | "Hotfix") {
        return None;
    }

    let is_msi = read_u32(hkey, "WindowsInstaller").unwrap_or(0) == 1 || is_guid_keyname(subname);
    let product_code = if is_guid_keyname(subname) {
        subname.to_string()
    } else {
        String::new()
    };

    Some(AppEntry {
        id: format!("{}|{}", source, subname),
        name,
        publisher: read_string(hkey, "Publisher"),
        version: read_string(hkey, "DisplayVersion"),
        install_date: read_string(hkey, "InstallDate"),
        install_location: expand_env(&read_string(hkey, "InstallLocation")),
        estimated_size_kb: read_u32(hkey, "EstimatedSize").unwrap_or(0) as u64,
        uninstall_string: read_string(hkey, "UninstallString"),
        quiet_uninstall_string: read_string(hkey, "QuietUninstallString"),
        display_icon: read_string(hkey, "DisplayIcon"),
        is_msi,
        product_code,
        arch: arch.to_string(),
        source: source.to_string(),
    })
}

/// 枚举已装应用清单。
pub fn list_installed_apps() -> Vec<AppEntry> {
    let mut apps: Vec<AppEntry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let path = to_wide(UNINSTALL_PATH);

    let targets: [(HKEY, REG_SAM_FLAGS, &str, &str); 3] = [
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY, "HKLM", "x64"),
        (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY, "HKLM-WOW", "x86"),
        (HKEY_CURRENT_USER, KEY_READ, "HKCU", "user"),
    ];

    for (hive, sam, source, arch) in targets {
        unsafe {
            let mut root = HKEY::default();
            if RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), 0, sam, &mut root) != ERROR_SUCCESS {
                continue;
            }
            for subname in enum_subkey_names(root) {
                let subw = to_wide(&subname);
                let mut sub = HKEY::default();
                if RegOpenKeyExW(root, PCWSTR(subw.as_ptr()), 0, sam, &mut sub) != ERROR_SUCCESS {
                    continue;
                }
                if let Some(entry) = read_app(sub, &subname, source, arch) {
                    let key = (
                        entry.name.to_lowercase(),
                        entry.version.clone(),
                        entry.publisher.clone(),
                    );
                    if seen.insert(key) {
                        apps.push(entry);
                    }
                }
                let _ = RegCloseKey(sub);
            }
            let _ = RegCloseKey(root);
        }
    }

    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_keyname_detection() {
        assert!(is_guid_keyname("{0F1B7A2C-1234-4567-89AB-CDEF01234567}"));
        assert!(!is_guid_keyname("Notepad++"));
        assert!(!is_guid_keyname("{too-short}"));
    }

    #[test]
    fn env_expansion() {
        std::env::set_var("WINTOP_TEST_ROOT", "C:\\Foo");
        assert_eq!(expand_env("%WINTOP_TEST_ROOT%\\bar"), "C:\\Foo\\bar");
        assert_eq!(expand_env("no vars here"), "no vars here");
        // 未知变量原样保留
        assert_eq!(expand_env("%NOPE_NOT_SET_XYZ%\\x"), "%NOPE_NOT_SET_XYZ%\\x");
    }

    #[test]
    fn enumerate_real_apps_smoke() {
        // 冒烟：真机应能枚举到若干应用，且每条都有非空名字。
        let apps = list_installed_apps();
        assert!(!apps.is_empty(), "应至少枚举到一个已装应用");
        assert!(apps.iter().all(|a| !a.name.is_empty()));
    }
}
