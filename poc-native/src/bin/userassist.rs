//! POC: 解码 UserAssist（HKCU）取应用的运行次数 + 最后使用时间。
//!
//! 无需管理员（读当前用户 HKCU）。验证点：
//!   1. 值名是 ROT13 编码的路径 —— 解码后是否是真实可执行路径。
//!   2. 值数据是 Win7+ 的 72 字节结构：RunCount@偏移4(u32)，LastExecuted FILETIME@偏移60(u64)。
//!   3. 解出的"次数/最后使用"对已知常用程序是否与实际相符。
//!
//! 这是 usage_stats.rs「最后使用时间」的主数据源之一（无需管理员，优于需管理员的 Prefetch）。

use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
};

// 现代 Windows 两个 UserAssist 子键：可执行程序 / 快捷方式
const GUIDS: [&str; 2] = [
    "{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}", // 程序
    "{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}", // 快捷方式
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// ROT13：只旋转 ASCII 字母，其余（数字/反斜杠/花括号/连字符）原样。
fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            _ => c,
        })
        .collect()
}

/// FILETIME(100ns since 1601) -> Unix 秒
fn filetime_to_unix(ft: u64) -> i64 {
    const EPOCH_DIFF: i64 = 11_644_473_600; // 1601-01-01 到 1970-01-01 的秒数
    (ft / 10_000_000) as i64 - EPOCH_DIFF
}

fn main() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut total = 0usize;
    let mut rows: Vec<(String, u32, i64)> = Vec::new();

    for guid in GUIDS {
        let path = format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\UserAssist\\{}\\Count",
            guid
        );
        let pathw = wide(&path);
        let mut hkey = HKEY::default();
        let r = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(pathw.as_ptr()), 0, KEY_READ, &mut hkey)
        };
        if r != ERROR_SUCCESS {
            println!("打开子键失败（{}）：{}", guid, r.0);
            continue;
        }

        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 2048];
            let mut name_len = name_buf.len() as u32;
            let mut data_buf = [0u8; 4096];
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
            total += 1;

            let raw_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let name = rot13(&raw_name);
            if name.starts_with("UEME_") {
                continue; // 聚合会话计数，不是应用
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
            let last_unix = if last_ft > 0 { filetime_to_unix(last_ft) } else { 0 };
            rows.push((name, run_count, last_unix));
        }
        unsafe {
            let _ = RegCloseKey(hkey);
        }
    }

    rows.sort_by(|a, b| b.2.cmp(&a.2)); // 最近使用在前
    println!("== UserAssist 条目 {}（含聚合），应用条目 {} ==\n", total, rows.len());
    println!("{:>6}  {:<12}  {}", "次数", "最后使用", "程序（ROT13 解码后）");
    for (name, count, last_unix) in rows.iter().take(30) {
        let when = if *last_unix > 0 {
            let days = (now - last_unix) / 86400;
            if days <= 0 {
                "今天".to_string()
            } else {
                format!("{} 天前", days)
            }
        } else {
            "—".to_string()
        };
        println!("{:>6}  {:<12}  {}", count, when, name);
    }
}
