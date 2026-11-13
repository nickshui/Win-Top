//! 应用图标提取：从 DisplayIcon / 安装目录定位可执行文件或图标文件，
//! 用 ExtractIconEx 取出 HICON，经 WIC 编码为 PNG data URL 供前端展示。
//!
//! 设计：批量接口一次处理整份应用清单；结果按 (display_icon, install_location)
//! 缓存，失败也缓存（避免每次刷新重复尝试）。提取过程在独立 MTA 线程执行，
//! 与 Tauri/WebView2 的 STA 套间隔离。

use std::collections::HashMap;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use serde::Deserialize;

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::{
    IWICBitmapFrameEncode, IWICImagingFactory, CLSID_WICImagingFactory, GUID_ContainerFormatPng,
    WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag2;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IStream, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

#[derive(Deserialize)]
pub struct IconRequest {
    pub id: String,
    pub display_icon: String,
    pub install_location: String,
}

static ICON_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn cache_key(display_icon: &str, install_location: &str) -> String {
    format!("{}\u{1}{}", display_icon.trim(), install_location.trim())
}

/// 解析 DisplayIcon（形如 `"C:\App\a.exe",0`、`C:\App\a.ico`）为 (路径, 图标索引)。
fn split_icon_ref(raw: &str) -> Option<(PathBuf, i32)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (path_part, index) = if let Some(rest) = s.strip_prefix('"') {
        match rest.split_once('"') {
            Some((p, tail)) => (
                p.to_string(),
                tail.trim()
                    .trim_start_matches(',')
                    .trim()
                    .parse::<i32>()
                    .unwrap_or(0),
            ),
            None => (rest.to_string(), 0),
        }
    } else if let Some((left, right)) = s.rsplit_once(',') {
        match right.trim().parse::<i32>() {
            Ok(idx) => (left.trim().to_string(), idx),
            Err(_) => (s.to_string(), 0),
        }
    } else {
        (s.to_string(), 0)
    };
    let expanded = crate::app_inventory::expand_env(path_part.trim());
    let path = PathBuf::from(expanded);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some((path, index))
    }
}

/// 找到可用于提取图标的源文件：优先 DisplayIcon 指向的文件，其次安装目录下的 exe。
fn icon_source(display_icon: &str, install_location: &str) -> Option<(PathBuf, i32)> {
    if let Some((path, index)) = split_icon_ref(display_icon) {
        if path.is_file() {
            return Some((path, index.max(0)));
        }
    }
    let loc_raw = install_location.trim().trim_matches('"');
    if !loc_raw.is_empty() {
        let loc = PathBuf::from(crate::app_inventory::expand_env(loc_raw));
        if loc.is_dir() {
            let mut exes: Vec<PathBuf> = std::fs::read_dir(&loc)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .and_then(|s| s.to_str())
                            .map(|s| s.eq_ignore_ascii_case("exe"))
                            .unwrap_or(false)
                })
                .take(64)
                .collect();
            exes.sort();
            if let Some(exe) = exes.into_iter().next() {
                return Some((exe, 0));
            }
        }
    }
    None
}

/// 用 WIC 把 HICON 编码为 PNG 字节。
unsafe fn encode_hicon_png(hicon: HICON) -> Option<Vec<u8>> {
    let factory: IWICImagingFactory = CoCreateInstance(
        &CLSID_WICImagingFactory,
        None::<&windows::core::IUnknown>,
        CLSCTX_INPROC_SERVER,
    )
    .ok()?;
    let bitmap = factory.CreateBitmapFromHICON(hicon).ok()?;
    let stream = factory.CreateStream().ok()?;

    let tmp = std::env::temp_dir().join(format!(
        "wintop-icon-{}-{}.png",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp_h = HSTRING::from(tmp.to_string_lossy().as_ref());
    stream.InitializeFromFilename(&tmp_h, GENERIC_WRITE.0).ok()?;

    let encoder = factory
        .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
        .ok()?;
    let stream_iface: IStream = stream.cast().ok()?;
    encoder.Initialize(&stream_iface, WICBitmapEncoderNoCache).ok()?;

    let mut frame: Option<IWICBitmapFrameEncode> = None;
    let mut props: Option<IPropertyBag2> = None;
    encoder.CreateNewFrame(&mut frame, &mut props).ok()?;
    let frame = frame?;
    frame.Initialize(None::<&IPropertyBag2>).ok()?;
    frame.WriteSource(&bitmap, std::ptr::null()).ok()?;
    frame.Commit().ok()?;
    encoder.Commit().ok()?;

    drop(frame);
    drop(stream_iface);
    drop(stream);
    drop(factory);

    let bytes = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    bytes.filter(|b| !b.is_empty())
}

unsafe fn extract_icon_png(path: &Path, index: i32) -> Option<Vec<u8>> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
    let mut icon = HICON::default();
    let mut count = ExtractIconExW(PCWSTR(wide.as_ptr()), index, Some(&mut icon), None, 1);
    if count == 0 || icon.0.is_null() {
        // 指定索引可能越界：退回第 0 个
        count = ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut icon), None, 1);
    }
    if count == 0 || icon.0.is_null() {
        return None;
    }
    let png = encode_hicon_png(icon);
    let _ = DestroyIcon(icon);
    png
}

/// 批量解析图标，返回 id → data URL（仅成功项）。
pub fn resolve_icons(items: Vec<IconRequest>) -> HashMap<String, String> {
    if items.is_empty() {
        return HashMap::new();
    }
    // 独立 MTA 线程内完成 COM/WIC 初始化
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut result = HashMap::new();
        for item in items {
            let key = cache_key(&item.display_icon, &item.install_location);
            let cached = ICON_CACHE
                .lock()
                .ok()
                .and_then(|c| c.get(&key).cloned());
            let data = match cached {
                Some(v) => v,
                None => {
                    let computed = icon_source(&item.display_icon, &item.install_location)
                        .and_then(|(path, idx)| extract_icon_png(&path, idx))
                        .map(|png| {
                            format!(
                                "data:image/png;base64,{}",
                                base64::engine::general_purpose::STANDARD.encode(png)
                            )
                        });
                    if let Ok(mut c) = ICON_CACHE.lock() {
                        c.insert(key, computed.clone());
                    }
                    computed
                }
            };
            if let Some(data) = data {
                result.insert(item.id, data);
            }
        }
        result
    })
    .join()
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::IconRequest;

    #[test]
    #[ignore = "真机冒烟：提取 notepad 图标"]
    fn extract_notepad_icon() {
        let items = vec![IconRequest {
            id: "np".into(),
            display_icon: r"C:\Windows\System32\notepad.exe".into(),
            install_location: String::new(),
        }];
        let map = super::resolve_icons(items);
        let data = map.get("np").expect("应能提取 notepad 图标");
        assert!(data.starts_with("data:image/png;base64,"));
        assert!(data.len() > 200);
        println!("icon data url length = {}", data.len());
    }
}
