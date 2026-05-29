//! 磁盘模块：
//!   - 逻辑分区容量：Win32 API（GetLogicalDriveStrings/GetDiskFreeSpaceEx/GetVolumeInformation），无需管理员
//!   - 物理磁盘健康：WMI Win32_DiskDrive（root\cimv2，无需管理员）。Status 字段反映 SMART 预测（OK/Degraded/Pred Fail）
//!   - 温度/磨损（MSFT_StorageReliabilityCounter）通常需管理员 + 仅 NVMe，后续迭代再加。

use serde::{Deserialize, Serialize};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
};

use wmi::{COMLibrary, Variant, WMIConnection};

#[derive(Serialize)]
pub struct VolumeInfo {
    pub drive: String,
    pub label: String,
    pub fs: String,
    pub drive_type: String,
    pub total: u64,
    pub free: u64,
    pub used_pct: f64,
}

#[derive(Serialize)]
pub struct PhysicalDisk {
    pub model: String,
    pub status: String,
    pub healthy: bool,
    pub size: u64,
    pub interface: String,
    pub media: String,
    pub serial: String,
    pub temperature: Option<u16>, // °C，仅提权 + 受支持的盘（多为 NVMe）
}

#[derive(Serialize)]
pub struct DiskReport {
    pub volumes: Vec<VolumeInfo>,
    pub disks: Vec<PhysicalDisk>,
    pub smart_note: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn list_volumes() -> Vec<VolumeInfo> {
    let mut out = Vec::new();
    let mut drives = [0u16; 256];
    let len = unsafe { GetLogicalDriveStringsW(Some(&mut drives)) };
    if len == 0 {
        return out;
    }

    for root in drives[..len as usize]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
    {
        let root_str = String::from_utf16_lossy(root); // 形如 "C:\\"
        let wroot = wide(&root_str);

        let drive_type = match unsafe { GetDriveTypeW(PCWSTR(wroot.as_ptr())) } {
            2 => "可移动",
            3 => "本地磁盘",
            4 => "网络",
            5 => "光驱",
            6 => "内存盘",
            _ => "未知",
        };

        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wroot.as_ptr()),
                None,
                Some(&mut total as *mut u64),
                Some(&mut free as *mut u64),
            )
        };
        // 空光驱 / 未插入可移动设备会失败，跳过
        if ok.is_err() || total == 0 {
            continue;
        }

        let mut label_buf = [0u16; 256];
        let mut fs_buf = [0u16; 64];
        let _ = unsafe {
            GetVolumeInformationW(
                PCWSTR(wroot.as_ptr()),
                Some(&mut label_buf),
                None,
                None,
                None,
                Some(&mut fs_buf),
            )
        };

        let used = total.saturating_sub(free);
        let used_pct = used as f64 / total as f64 * 100.0;
        let label = buf_to_string(&label_buf);

        out.push(VolumeInfo {
            drive: root_str.trim_end_matches('\\').to_string(),
            label: if label.is_empty() {
                "本地磁盘".to_string()
            } else {
                label
            },
            fs: buf_to_string(&fs_buf),
            drive_type: drive_type.to_string(),
            total,
            free,
            used_pct,
        });
    }
    out
}

#[derive(Deserialize)]
#[serde(rename = "Win32_DiskDrive")]
#[serde(rename_all = "PascalCase")]
struct Win32DiskDrive {
    model: Option<String>,
    status: Option<String>,
    size: Option<u64>,
    index: Option<u32>,
    interface_type: Option<String>,
    media_type: Option<String>,
    serial_number: Option<String>,
}

// 物理盘引用：仅需 DeviceId + __Path（associators 需要对象路径）
#[derive(Deserialize)]
#[serde(rename = "MSFT_PhysicalDisk")]
#[allow(non_snake_case)]
struct PhysDiskRef {
    DeviceId: Option<String>,
    __Path: Option<String>,
}

fn variant_to_u16(v: Option<&Variant>) -> Option<u16> {
    match v {
        Some(Variant::UI1(n)) => Some(*n as u16),
        Some(Variant::UI2(n)) => Some(*n),
        Some(Variant::I2(n)) => Some(*n as u16),
        Some(Variant::I4(n)) => Some(*n as u16),
        _ => None,
    }
}

// 温度需经 ASSOCIATORS（直查 MSFT_StorageReliabilityCounter 返回 0 行）。
// 返回 DeviceId(==Win32_DiskDrive.Index) -> 温度°C。需管理员，否则为空。
fn disk_temperatures(com: COMLibrary) -> std::collections::HashMap<u32, u16> {
    use std::collections::HashMap;
    let mut map = HashMap::new();
    let con = match WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage", com) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let disks: Vec<PhysDiskRef> = match con.query() {
        Ok(d) => d,
        Err(_) => return map,
    };
    for d in disks {
        let id = match d.DeviceId.as_deref().and_then(|s| s.parse::<u32>().ok()) {
            Some(i) => i,
            None => continue,
        };
        let path = match &d.__Path {
            Some(p) => p,
            None => continue,
        };
        let query = format!(
            "ASSOCIATORS OF {{{}}} WHERE ResultClass = MSFT_StorageReliabilityCounter",
            path
        );
        if let Ok(rows) = con.raw_query::<HashMap<String, Variant>>(&query) {
            if let Some(temp) = rows.first().and_then(|r| variant_to_u16(r.get("Temperature"))) {
                if temp > 0 {
                    map.insert(id, temp);
                }
            }
        }
    }
    map
}

fn list_physical_disks() -> Result<Vec<PhysicalDisk>, String> {
    // 在全新线程里执行，避免与 Tauri/WebView2 已初始化的 STA 套间冲突
    // （否则 COMLibrary::new() 会因套间模型不一致报 RPC_E_CHANGED_MODE 0x80010106）
    std::thread::spawn(|| -> Result<Vec<PhysicalDisk>, String> {
        let com = COMLibrary::new().map_err(|e| e.to_string())?;
        let con = WMIConnection::new(com).map_err(|e| e.to_string())?;
        let results: Vec<Win32DiskDrive> = con.query().map_err(|e| e.to_string())?;

        // 温度：经 ASSOCIATORS 从 MSFT_PhysicalDisk 关联到可靠性计数器。
        // 需管理员；失败/无数据时静默降级（temperature = None）。
        let temps = disk_temperatures(com);

        Ok(results
            .into_iter()
            .map(|d| {
                let status = d.status.unwrap_or_else(|| "Unknown".to_string());
                let healthy = status.eq_ignore_ascii_case("OK");
                let temperature = d.index.and_then(|i| temps.get(&i).copied());
                PhysicalDisk {
                    model: d.model.unwrap_or_default().trim().to_string(),
                    status,
                    healthy,
                    size: d.size.unwrap_or(0),
                    interface: d.interface_type.unwrap_or_default(),
                    media: d.media_type.unwrap_or_default(),
                    serial: d.serial_number.unwrap_or_default().trim().to_string(),
                    temperature,
                }
            })
            .collect())
    })
    .join()
    .map_err(|_| "WMI 查询线程异常退出".to_string())?
}

pub fn report() -> DiskReport {
    let volumes = list_volumes();
    let (disks, smart_note) = match list_physical_disks() {
        Ok(d) => (d, String::new()),
        Err(e) => (
            Vec::new(),
            format!("物理磁盘信息获取失败（WMI）：{}", e),
        ),
    };
    DiskReport {
        volumes,
        disks,
        smart_note,
    }
}
