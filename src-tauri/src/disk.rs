//! 磁盘模块：
//!   - 逻辑分区容量：Win32 API（GetLogicalDriveStrings/GetDiskFreeSpaceEx/GetVolumeInformation），无需管理员
//!   - 物理磁盘健康：WMI Win32_DiskDrive（root\cimv2）+ MSFT_StorageReliabilityCounter（需管理员）
//!   - 寿命/磨损：NVMe 健康日志（IOCTL_STORAGE_QUERY_PROPERTY）与 SATA SMART（SMART_RCV_DRIVE_DATA）
//!   - 卷→物理盘映射：Win32_DiskDriveToDiskPartition / Win32_LogicalDiskToPartition

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    NVMeDataTypeLogPage, PropertyStandardQuery, ProtocolTypeNvme,
    StorageDeviceProtocolSpecificProperty, SMART_GET_VERSION, SMART_RCV_DRIVE_DATA,
    IOCTL_STORAGE_QUERY_PROPERTY,
};
use windows::Win32::System::IO::DeviceIoControl;

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

#[derive(Serialize, Clone)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    pub percentage_used: u8,
    pub available_spare: u8,
    pub available_spare_threshold: u8,
    pub data_units_read: u64,
    pub data_units_written: u64,
    pub power_cycles: u64,
    pub power_on_hours: u64,
    pub unsafe_shutdowns: u64,
    pub media_errors: u64,
    pub temperature_c: Option<u16>,
}

#[derive(Serialize, Clone)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u8,
    pub worst: u8,
    pub raw: u64,
}

#[derive(Serialize)]
pub struct PhysicalDisk {
    pub index: u32,
    pub model: String,
    pub status: String,
    pub healthy: bool,
    pub size: u64,
    pub interface: String,
    pub media: String,
    pub serial: String,
    pub firmware: String,
    pub temperature: Option<u16>,
    pub temperature_max: Option<u16>,
    pub power_on_hours: Option<u64>,
    /// 磨损百分比（0-100，越大越旧）；来源：NVMe 健康日志 / WMI Wear / SATA SMART
    pub wear: Option<u32>,
    pub read_errors: Option<u64>,
    pub write_errors: Option<u64>,
    pub power_cycles: Option<u64>,
    pub load_unload_cycles: Option<u64>,
    pub manufacture_date: Option<String>,
    /// 该物理盘承载的分区盘符
    pub volumes: Vec<String>,
    pub nvme: Option<NvmeHealth>,
    pub smart_attributes: Vec<SmartAttribute>,
    /// 本盘数据来源说明（用户可见）
    pub data_source: String,
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
        let root_str = String::from_utf16_lossy(root);
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
            label: if label.is_empty() { "本地磁盘".to_string() } else { label },
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
    firmware_revision: Option<String>,
}

// 物理盘引用：仅需 DeviceId + __Path（associators 需要对象路径）
#[derive(Deserialize)]
#[serde(rename = "MSFT_PhysicalDisk")]
#[allow(non_snake_case)]
struct PhysDiskRef {
    DeviceId: Option<String>,
    __Path: Option<String>,
}

fn variant_u64(v: Option<&Variant>) -> Option<u64> {
    match v {
        Some(Variant::UI1(n)) => Some(*n as u64),
        Some(Variant::UI2(n)) => Some(*n as u64),
        Some(Variant::UI4(n)) => Some(*n as u64),
        Some(Variant::UI8(n)) => Some(*n),
        Some(Variant::I1(n)) => Some((*n).max(0) as u64),
        Some(Variant::I2(n)) => Some((*n).max(0) as u64),
        Some(Variant::I4(n)) => Some((*n).max(0) as u64),
        Some(Variant::I8(n)) => Some((*n).max(0) as u64),
        Some(Variant::String(s)) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn variant_string(v: Option<&Variant>) -> Option<String> {
    match v {
        Some(Variant::String(s)) => Some(s.clone()),
        Some(v) => variant_u64(Some(v)).map(|n| n.to_string()),
        None => None,
    }
}

#[derive(Clone, Default)]
struct ReliabilityInfo {
    temperature: Option<u16>,
    temperature_max: Option<u16>,
    power_on_hours: Option<u64>,
    wear: Option<u32>,
    read_errors: Option<u64>,
    write_errors: Option<u64>,
    start_stop: Option<u64>,
    load_unload: Option<u64>,
    manufacture_date: Option<String>,
}

/// 从 MSFT_StorageReliabilityCounter（经 ASSOCIATORS）提取寿命相关字段。
fn disk_reliability(com: COMLibrary) -> HashMap<u32, ReliabilityInfo> {
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
            if let Some(r) = rows.first() {
                let mut info = ReliabilityInfo::default();
                info.temperature = variant_u64(r.get("Temperature")).map(|v| v as u16).filter(|v| *v > 0);
                info.temperature_max = variant_u64(r.get("TemperatureMax")).map(|v| v as u16).filter(|v| *v > 0);
                info.power_on_hours = variant_u64(r.get("PowerOnHours")).filter(|v| *v > 0);
                info.wear = variant_u64(r.get("Wear")).map(|v| v as u32).filter(|v| *v <= 100);
                info.read_errors = variant_u64(r.get("ReadErrorsTotal"));
                info.write_errors = variant_u64(r.get("WriteErrorsTotal"));
                info.start_stop = variant_u64(r.get("StartStopCycles"));
                info.load_unload = variant_u64(r.get("LoadUnloadCycles"));
                info.manufacture_date = variant_string(r.get("ManufactureDate"));
                map.insert(id, info);
            }
        }
    }
    map
}

/// 从关联类构建「物理盘序号 → 盘符列表」映射。
fn disk_volume_map(con: &WMIConnection) -> HashMap<u32, Vec<String>> {
    let mut part_to_disk: HashMap<String, u32> = HashMap::new();
    let mut disk_to_letters: HashMap<u32, Vec<String>> = HashMap::new();

    let extract_device_id = |s: &str| -> Option<String> {
        let idx = s.find("DeviceID=")?;
        let rest = &s[idx + "DeviceID=".len()..];
        let start = rest.find('"')? + 1;
        let end = rest[start..].find('"')? + start;
        Some(rest[start..end].to_string())
    };
    let extract_physical = |s: &str| -> Option<u32> {
        let upper = s.to_uppercase();
        let idx = upper.find("PHYSICALDRIVE")?;
        let digits: String = upper[idx + "PHYSICALDRIVE".len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<u32>().ok()
    };

    if let Ok(rows) = con
        .raw_query::<HashMap<String, Variant>>("SELECT * FROM Win32_DiskDriveToDiskPartition")
    {
        for row in rows {
            let ante = variant_string(row.get("Antecedent")).unwrap_or_default();
            let dep = variant_string(row.get("Dependent")).unwrap_or_default();
            if let (Some(disk), Some(part)) = (extract_physical(&ante), extract_device_id(&dep)) {
                part_to_disk.insert(part, disk);
            }
        }
    }
    if let Ok(rows) = con
        .raw_query::<HashMap<String, Variant>>("SELECT * FROM Win32_LogicalDiskToPartition")
    {
        for row in rows {
            let ante = variant_string(row.get("Antecedent")).unwrap_or_default();
            let dep = variant_string(row.get("Dependent")).unwrap_or_default();
            let (Some(part), Some(letter)) = (extract_device_id(&ante), extract_device_id(&dep)) else {
                continue;
            };
            if let Some(disk) = part_to_disk.get(&part) {
                disk_to_letters.entry(*disk).or_default().push(letter);
            }
        }
    }
    for letters in disk_to_letters.values_mut() {
        letters.sort();
        letters.dedup();
    }
    disk_to_letters
}

// ---------------------------------------------------------------------------
// NVMe 健康日志（IOCTL_STORAGE_QUERY_PROPERTY）
// ---------------------------------------------------------------------------

fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}
fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}
fn read_u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off], buf[off + 1], buf[off + 2], buf[off + 3],
        buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7],
    ])
}

fn open_physical_drive(index: u32, write_access: bool) -> Option<HANDLE> {
    let path = format!("\\\\.\\PHYSICALDRIVE{}", index);
    let w = wide(&path);
    let mut access = GENERIC_READ.0;
    if write_access {
        access |= GENERIC_WRITE.0;
    }
    unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()
    }
}

/// NVMe SMART/Health Information Log（Log Page 0x02）。
/// 输入：STORAGE_PROPERTY_QUERY 头（8B）+ STORAGE_PROTOCOL_SPECIFIC_DATA（40B）；
/// 输出：同样的描述头 + 512 字节健康日志。
fn nvme_health(index: u32) -> Option<NvmeHealth> {
    let handle = open_physical_drive(index, false)?;
    let result = (|| -> Option<NvmeHealth> {
        let spec_off = 8usize;
        let mut input = vec![0u8; 8 + 40];
        input[0..4].copy_from_slice(&(StorageDeviceProtocolSpecificProperty.0 as u32).to_le_bytes());
        input[4..8].copy_from_slice(&(PropertyStandardQuery.0 as u32).to_le_bytes());
        input[8..12].copy_from_slice(&(ProtocolTypeNvme.0 as u32).to_le_bytes());
        input[12..16].copy_from_slice(&(NVMeDataTypeLogPage.0 as u32).to_le_bytes());
        input[16..20].copy_from_slice(&0x02u32.to_le_bytes()); // NVMe Log Page: SMART/Health
        input[20..24].copy_from_slice(&0u32.to_le_bytes()); // SubValue
        input[24..28].copy_from_slice(&40u32.to_le_bytes()); // ProtocolDataOffset
        input[28..32].copy_from_slice(&512u32.to_le_bytes()); // ProtocolDataLength
        let mut out = vec![0u8; spec_off + 40 + 512 + 64];
        let mut returned = 0u32;
        unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(input.as_ptr() as *const core::ffi::c_void),
                input.len() as u32,
                Some(out.as_mut_ptr() as *mut core::ffi::c_void),
                out.len() as u32,
                Some(&mut returned),
                None,
            )
            .ok()?;
        }
        if (returned as usize) < spec_off + 40 + 176 {
            return None;
        }
        let proto_off = read_u32_le(&out, spec_off + 16) as usize;
        let data = spec_off + proto_off;
        if data + 176 > out.len() {
            return None;
        }
        let temperature = read_u16_le(&out, data + 1);
        Some(NvmeHealth {
            critical_warning: out[data],
            temperature_c: if temperature > 273 && temperature < 400 {
                Some(temperature - 273)
            } else {
                None
            },
            available_spare: out[data + 3],
            available_spare_threshold: out[data + 4],
            percentage_used: out[data + 5],
            data_units_read: read_u64_le(&out, data + 32),
            data_units_written: read_u64_le(&out, data + 48),
            power_cycles: read_u64_le(&out, data + 112),
            power_on_hours: read_u64_le(&out, data + 128),
            unsafe_shutdowns: read_u64_le(&out, data + 144),
            media_errors: read_u64_le(&out, data + 160),
        })
    })();
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

// ---------------------------------------------------------------------------
// SATA SMART（SMART_RCV_DRIVE_DATA，需管理员）
// ---------------------------------------------------------------------------

fn smart_attr_name(id: u8) -> &'static str {
    match id {
        1 => "底层读取错误率",
        5 => "重映射扇区计数",
        9 => "通电时长",
        12 => "通电次数",
        177 => "磨损均衡计数",
        187 => "不可校正错误",
        188 => "命令超时",
        190 => "气流温度",
        194 => "温度",
        196 => "重映射事件计数",
        197 => "待映射扇区",
        198 => "离线不可校正",
        199 => "接口 CRC 错误",
        231 => "SSD 剩余寿命",
        233 => "介质磨损指标",
        _ => "厂商特定",
    }
}

#[derive(Default)]
struct SmartInfo {
    attributes: Vec<SmartAttribute>,
    power_on_hours: Option<u64>,
    wear: Option<u32>,
    temperature: Option<u16>,
}

fn sata_smart(index: u32) -> SmartInfo {
    let mut info = SmartInfo::default();
    let handle = match open_physical_drive(index, true) {
        Some(h) => h,
        None => return info,
    };

    unsafe {
        // 1) 能力检测（GET VERSION）
        let mut ver = [0u8; 24];
        let mut returned = 0u32;
        let ok = DeviceIoControl(
            handle,
            SMART_GET_VERSION,
            None,
            0,
            Some(ver.as_mut_ptr() as *mut core::ffi::c_void),
            ver.len() as u32,
            Some(&mut returned),
            None,
        )
        .is_ok();
        // fCapabilities 在偏移 4；CAP_SMART_CMD = 0x4
        if !ok || returned < 8 || (read_u32_le(&ver, 4) & 0x4) == 0 {
            let _ = CloseHandle(handle);
            return info;
        }

        // 2) READ ATTRIBUTES：bFeatures=0xD0, bCommand=0xB0, LBA=0xC24F
        let mut input = vec![0u8; 36 + 512];
        input[0..4].copy_from_slice(&512u32.to_le_bytes()); // cBufferSize
        let regs = 4usize; // IDEREGS 起点
        input[regs] = 0xD0; // bFeaturesReg: SMART_READ_DATA
        input[regs + 1] = 1; // bSectorCountReg
        input[regs + 2] = 1; // bSectorNumberReg
        input[regs + 3] = 0x4F; // bCylLowReg
        input[regs + 4] = 0xC2; // bCylHighReg
        input[regs + 5] = 0xA0; // bDriveHeadReg
        input[regs + 6] = 0xB0; // bCommandReg: SMART

        let mut out = vec![0u8; 12 + 512];
        let mut returned2 = 0u32;
        let ok2 = DeviceIoControl(
            handle,
            SMART_RCV_DRIVE_DATA,
            Some(input.as_ptr() as *const core::ffi::c_void),
            input.len() as u32,
            Some(out.as_mut_ptr() as *mut core::ffi::c_void),
            out.len() as u32,
            Some(&mut returned2),
            None,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if !ok2 || (returned2 as usize) < 12 + 362 {
            return info;
        }

        // SENDCMDOUTPARAMS: 4B 大小 + 8B DRIVERSTATUS + bBuffer（12 字节起为属性页）
        // DRIVERSTATUS.bDriverError 位于偏移 4；非 0 表示命令失败
        if out[4] != 0 {
            return info;
        }
        let data = &out[12..12 + 512];
        for i in 0..30 {
            let off = 2 + i * 12;
            let id = data[off];
            if id == 0 {
                continue;
            }
            let value = data[off + 3];
            let worst = data[off + 4];
            let mut raw_bytes = [0u8; 6];
            raw_bytes.copy_from_slice(&data[off + 5..off + 11]);
            let raw = u32::from_le_bytes([raw_bytes[0], raw_bytes[1], raw_bytes[2], raw_bytes[3]]) as u64
                | ((raw_bytes[4] as u64) << 32)
                | ((raw_bytes[5] as u64) << 40);
            info.attributes.push(SmartAttribute {
                id,
                name: smart_attr_name(id).to_string(),
                value,
                worst,
                raw,
            });
            match id {
                9 => {
                    let hours = u32::from_le_bytes([raw_bytes[0], raw_bytes[1], raw_bytes[2], raw_bytes[3]]) as u64;
                    if hours > 0 {
                        info.power_on_hours = Some(hours);
                    }
                }
                231 | 233 => {
                    if value > 0 && value <= 100 {
                        info.wear = Some(100u32.saturating_sub(value as u32));
                    }
                }
                177 => {
                    // Wear_Leveling_Count 多数厂商从 100 递减；仅在未取到磨损值时作为兜底
                    if info.wear.is_none() && value > 0 && value <= 100 {
                        info.wear = Some(100u32.saturating_sub(value as u32));
                    }
                }
                190 | 194 => {
                    let t = raw_bytes[0] as u16;
                    if t > 0 && t <= 120 {
                        info.temperature = Some(t);
                    }
                }
                _ => {}
            }
        }
    }
    info
}

// ---------------------------------------------------------------------------
// 磁盘 SMART 数据缓存（NVMe/SATA 查询较重，60 秒内复用）
// ---------------------------------------------------------------------------

struct SmartCacheEntry {
    at: Instant,
    nvme: Option<NvmeHealth>,
    info: SmartInfo,
}

static SMART_CACHE: LazyLock<Mutex<HashMap<u32, SmartCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn smart_for(index: u32) -> (Option<NvmeHealth>, SmartInfo) {
    if let Ok(cache) = SMART_CACHE.lock() {
        if let Some(entry) = cache.get(&index) {
            if entry.at.elapsed() < Duration::from_secs(60) {
                return (entry.nvme.clone(), SmartInfo {
                    attributes: entry.info.attributes.clone(),
                    power_on_hours: entry.info.power_on_hours,
                    wear: entry.info.wear,
                    temperature: entry.info.temperature,
                });
            }
        }
    }
    let nvme = nvme_health(index);
    let mut info = SmartInfo::default();
    if nvme.is_none() {
        info = sata_smart(index);
    }
    if let Ok(mut cache) = SMART_CACHE.lock() {
        cache.insert(
            index,
            SmartCacheEntry {
                at: Instant::now(),
                nvme: nvme.clone(),
                info: SmartInfo {
                    attributes: info.attributes.clone(),
                    power_on_hours: info.power_on_hours,
                    wear: info.wear,
                    temperature: info.temperature,
                },
            },
        );
    }
    (nvme, info)
}

// ---------------------------------------------------------------------------

fn list_physical_disks() -> Result<Vec<PhysicalDisk>, String> {
    // 在全新线程里执行，避免与 Tauri/WebView2 已初始化的 STA 套间冲突
    // （否则 COMLibrary::new() 会因套间模型不一致报 RPC_E_CHANGED_MODE 0x80010106）
    std::thread::spawn(|| -> Result<Vec<PhysicalDisk>, String> {
        let com = COMLibrary::new().map_err(|e| e.to_string())?;
        let con = WMIConnection::new(com).map_err(|e| e.to_string())?;
        let results: Vec<Win32DiskDrive> = con.query().map_err(|e| e.to_string())?;
        let reliability = disk_reliability(com);
        let volume_map = disk_volume_map(&con);

        Ok(results
            .into_iter()
            .map(|d| {
                let index = d.index.unwrap_or(0);
                let rel = reliability.get(&index).cloned().unwrap_or_default();
                let (nvme, smart) = smart_for(index);

                let temperature = nvme
                    .as_ref()
                    .and_then(|n| n.temperature_c)
                    .or(rel.temperature)
                    .or(smart.temperature);
                let power_on_hours = nvme
                    .as_ref()
                    .map(|n| n.power_on_hours)
                    .filter(|v| *v > 0)
                    .or(rel.power_on_hours)
                    .or(smart.power_on_hours);
                let wear = nvme
                    .as_ref()
                    .map(|n| n.percentage_used as u32)
                    .filter(|v| *v <= 100)
                    .or(rel.wear)
                    .or(smart.wear);
                let power_cycles = nvme
                    .as_ref()
                    .map(|n| n.power_cycles)
                    .filter(|v| *v > 0)
                    .or(rel.start_stop);
                let load_unload = rel.load_unload;

                let data_source = if nvme.is_some() {
                    "NVMe 健康日志"
                } else if !smart.attributes.is_empty() {
                    "SATA SMART 属性"
                } else if rel.power_on_hours.is_some() || rel.wear.is_some() {
                    "WMI 可靠性计数器"
                } else {
                    "仅基本信息（需管理员或该盘不支持 SMART）"
                };

                let status = d.status.unwrap_or_else(|| "Unknown".to_string());
                let healthy = status.eq_ignore_ascii_case("OK");
                PhysicalDisk {
                    index,
                    model: d.model.unwrap_or_default().trim().to_string(),
                    status,
                    healthy,
                    size: d.size.unwrap_or(0),
                    interface: d.interface_type.unwrap_or_default(),
                    media: d.media_type.unwrap_or_default(),
                    serial: d.serial_number.unwrap_or_default().trim().to_string(),
                    firmware: d.firmware_revision.unwrap_or_default().trim().to_string(),
                    temperature,
                    temperature_max: rel.temperature_max,
                    power_on_hours,
                    wear,
                    read_errors: rel.read_errors.filter(|v| *v > 0),
                    write_errors: rel.write_errors.filter(|v| *v > 0),
                    power_cycles,
                    load_unload_cycles: load_unload,
                    manufacture_date: rel.manufacture_date,
                    volumes: volume_map.get(&index).cloned().unwrap_or_default(),
                    nvme,
                    smart_attributes: smart.attributes,
                    data_source: data_source.to_string(),
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
        Err(e) => (Vec::new(), format!("物理磁盘信息获取失败（WMI）：{}", e)),
    };
    DiskReport {
        volumes,
        disks,
        smart_note,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "真机冒烟：读取物理磁盘与 SMART"]
    fn disk_report_smoke() {
        let r = super::report();
        println!("volumes={} disks={} note={:?}", r.volumes.len(), r.disks.len(), r.smart_note);
        for d in &r.disks {
            println!(
                "disk{} {} | temp={:?} max={:?} hours={:?} wear={:?} volumes={:?} src={} nvme={} attrs={}",
                d.index, d.model, d.temperature, d.temperature_max, d.power_on_hours, d.wear,
                d.volumes, d.data_source, d.nvme.is_some(), d.smart_attributes.len()
            );
        }
        assert!(!r.volumes.is_empty());
        assert!(!r.disks.is_empty());
    }
}
