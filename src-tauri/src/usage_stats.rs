//! 使用度：从 UserAssist（HKCU，值名 ROT13、数据 72 字节结构）解出应用的运行次数与
//! 最后使用时间。无需管理员。这是 app_score「价值」维度的主数据源（POC 已真机验证）。
//!
//! 坑：RegEnumValueW 的数据缓冲若太小会返回 ERROR_MORE_DATA 导致枚举静默截断——用 8KB。

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
};

use crate::app_inventory::to_wide;

// 现代 Windows 两个 UserAssist 子键：可执行程序 / 快捷方式
const GUIDS: [&str; 2] = [
    "{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}",
    "{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}",
];

#[derive(Serialize, Clone)]
pub struct UsageStat {
    pub last_used_secs: Option<i64>,
    pub run_count: u32,
    pub source: String, // userassist | none
}

impl UsageStat {
    fn none() -> Self {
        UsageStat {
            last_used_secs: None,
            run_count: 0,
            source: "none".into(),
        }
    }
}

/// 解码后的 UserAssist 索引，供批量查询复用（只构建一次）。
pub struct UsageIndex {
    /// (解码后路径的小写, 运行次数, 最后使用 Unix 秒)
    entries: Vec<(String, u32, i64)>,
}

fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            _ => c,
        })
        .collect()
}

fn filetime_to_unix(ft: u64) -> i64 {
    const EPOCH_DIFF: i64 = 11_644_473_600;
    (ft / 10_000_000) as i64 - EPOCH_DIFF
}

fn compact(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 构建 UserAssist 索引（读两处子键、ROT13 解名、解结构）。
pub fn build_index() -> UsageIndex {
    let mut entries = Vec::new();
    for guid in GUIDS {
        let path = format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\UserAssist\\{}\\Count",
            guid
        );
        let pathw = to_wide(&path);
        let mut hkey = HKEY::default();
        let opened =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(pathw.as_ptr()), 0, KEY_READ, &mut hkey) };
        if opened != ERROR_SUCCESS {
            continue;
        }
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 2048];
            let mut name_len = name_buf.len() as u32;
            let mut data_buf = [0u8; 8192];
            let mut data_len = data_buf.len() as u32;
            let r = unsafe {
                RegEnumValueW(
                    hkey,
                    index,
                    PWSTR(name_buf.as_mut_ptr()),
                    &mut name_len,
                    None,
                    None,
                    Some(data_buf.as_mut_ptr()),
                    Some(&mut data_len),
                )
            };
            if r == ERROR_NO_MORE_ITEMS || r != ERROR_SUCCESS {
                break;
            }
            index += 1;

            let raw = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let name = rot13(&raw);
            if name.starts_with("UEME_") {
                continue; // 聚合会话计数
            }
            let dl = data_len as usize;
            let run_count = if dl >= 8 {
                u32::from_le_bytes([data_buf[4], data_buf[5], data_buf[6], data_buf[7]])
            } else {
                0
            };
            let last_ft = if dl >= 68 {
                u64::from_le_bytes([
                    data_buf[60], data_buf[61], data_buf[62], data_buf[63], data_buf[64],
                    data_buf[65], data_buf[66], data_buf[67],
                ])
            } else {
                0
            };
            let last = if last_ft > 0 { filetime_to_unix(last_ft) } else { 0 };
            entries.push((name.to_lowercase(), run_count, last));
        }
        unsafe {
            let _ = RegCloseKey(hkey);
        }
    }
    UsageIndex { entries }
}

/// 测试用：从给定条目直接构造索引（供本 crate 其它模块的测试复用）。
#[cfg(test)]
pub(crate) fn index_from(entries: Vec<(String, u32, i64)>) -> UsageIndex {
    UsageIndex { entries }
}

impl UsageIndex {
    #[allow(dead_code)] // 供测试与未来调用方使用
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 查询某应用的使用度。三级匹配、取最近一次使用：
    ///   ① 条目路径落在安装目录内（最准）；
    ///   ② 基名紧凑名 == 产品紧凑名；
    ///   ③ 条目含发行商 token（如 WPS 的 UserAssist 名是 ProgID "Kingsoft.Office.KPrometheus"，
    ///      靠发行商 "kingsoft" 命中）。③偏“最近用过”是安全方向：宁可少推荐卸载，不误报“没用过”。
    pub fn lookup(&self, roots: &[String], name: &str, publisher: &str) -> UsageStat {
        let roots_l: Vec<String> = roots
            .iter()
            .filter(|r| r.len() >= 4)
            .map(|r| r.to_lowercase())
            .collect();

        let mut best: Option<(u32, i64)> = None;
        let merge = |best: &mut Option<(u32, i64)>, c: u32, l: i64| {
            *best = Some(match *best {
                Some((bc, bl)) => (bc.max(c), bl.max(l)),
                None => (c, l),
            });
        };

        // ① 路径落在安装目录内
        if !roots_l.is_empty() {
            for (p, c, l) in &self.entries {
                if roots_l.iter().any(|r| p.contains(r)) {
                    merge(&mut best, *c, *l);
                }
            }
        }
        // ② 基名紧凑名 == 产品紧凑名（不短路，与①②③一起取全局最近）
        let nc = compact(name);
        if nc.len() >= 4 {
            for (p, c, l) in &self.entries {
                let seg = p.rsplit(['\\', '/']).next().unwrap_or(p);
                let seg = seg.rsplit_once('.').map(|(a, _)| a).unwrap_or(seg);
                if compact(seg) == nc {
                    merge(&mut best, *c, *l);
                }
            }
        }
        // ③ 发行商 token：取全局最近一次。宁可偏“最近用过”（安全方向：不误报“没用过”、
        //    不误推卸载）。代价是同发行商多应用会互相沾染“最近使用”，属可接受的保守取舍。
        for pt in publisher_tokens(publisher) {
            for (p, c, l) in &self.entries {
                if p.contains(&pt) {
                    merge(&mut best, *c, *l);
                }
            }
        }
        match best {
            Some((c, l)) => UsageStat {
                last_used_secs: if l > 0 { Some(l) } else { None },
                run_count: c,
                source: "userassist".into(),
            },
            None => UsageStat::none(),
        }
    }
}

/// 发行商中有区分度的 token（小写、长度≥5、排除超通用词）。
fn publisher_tokens(publisher: &str) -> Vec<String> {
    const SKIP: &[&str] = &[
        "corp", "corporation", "company", "limited", "technology", "technologies", "software",
        "systems", "solutions", "group", "team", "studio", "microsoft", "google",
    ];
    publisher
        .split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= 5 && !SKIP.contains(&t.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_roundtrip() {
        assert_eq!(rot13("Uryyb"), "Hello");
        assert_eq!(rot13("C:\\Cebtenz"), "P:\\Program"); // 反斜杠/冒号不变，字母旋转
        assert_eq!(rot13(rot13("MixedCase123").as_str()), "MixedCase123");
    }

    #[test]
    fn filetime_conversion() {
        // 2023-01-15 的 FILETIME
        let unix = 1_673_778_600i64;
        let ft = (unix as u64 + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_to_unix(ft), unix);
    }

    #[test]
    fn index_builds_on_real_machine() {
        // 真机应能解出若干条目（无管理员）。
        let idx = build_index();
        assert!(!idx.is_empty(), "UserAssist 应至少解出一条应用记录");
    }
}
