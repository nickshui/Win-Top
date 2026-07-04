//! MFT 直接扫描器
//!
//! 通过读取 NTFS 主文件表（MFT）实现超快速全卷文件枚举，
//! 速度远快于逐目录 `std::fs` 递归。原理类似 WizTree / Everything。
//!
//! ## 方案
//! 使用 `DeviceIoControl` + `FSCTL_GET_NTFS_VOLUME_DATA` 获取卷信息，
//! 再逐个通过 `FSCTL_GET_NTFS_FILE_RECORD` 读取每个 MFT 记录，
//! 解析其中的 `$FILE_NAME` 属性获取文件名 / 父目录 / 时间戳，
//! 解析 `$DATA` 属性（resident / non-resident）获取真实文件大小。
//!
//! 最后在内存中重建目录树、自底向上累加每个目录的总占用，
//! 并通过 `BinaryHeap` 输出 top_n 个最大文件（定容堆，不收集全部再排序）。

use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::mem;

use crate::disk_usage;
use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_GET_NTFS_FILE_RECORD, NTFS_VOLUME_DATA_BUFFER,
    NTFS_FILE_RECORD_INPUT_BUFFER,
};

// ─── 小工具 ────────────────────────────────────────────────────────────

#[inline]
fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[inline]
fn read_u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

/// 读取 48 位小端整数（NTFS 文件参考号下半部分）
#[inline]
fn read_u48_le(buf: &[u8], off: usize) -> u64 {
    (buf[off] as u64)
        | ((buf[off + 1] as u64) << 8)
        | ((buf[off + 2] as u64) << 16)
        | ((buf[off + 3] as u64) << 24)
        | ((buf[off + 4] as u64) << 32)
        | ((buf[off + 5] as u64) << 40)
}

/// Windows FILETIME → Unix 秒
fn filetime_to_unix(ft: u64) -> u64 {
    const WINDOWS_EPOCH: u64 = 116444736000000000;
    if ft > WINDOWS_EPOCH {
        (ft - WINDOWS_EPOCH) / 10_000_000
    } else {
        0
    }
}

// ─── MFT 条目 ──────────────────────────────────────────────────────────

/// 每次 $FILE_NAME 属性对应一个条目（硬链接会生成多条）
#[derive(Clone, Debug)]
struct MftEntry {
    frn: u64,            // 本条所在的 MFT 记录号（低 48 位）
    parent_frn: u64,     // 父目录记录号（低 48 位）
    name: String,        // 文件名（UTF-8）
    size: u64,           // 文件大小（逻辑大小，等于 metadata().len()）
    is_dir: bool,        // 是否为目录
    modified_secs: u64,  // 最后修改时间（Unix 秒）
}

// ─── BinaryHeap 包装（最小堆，固定容量保留 top_n） ─────────────────────

#[derive(Clone, Debug)]
struct HeapFile {
    size: u64,
    path: String,
    modified_secs: u64,
}

impl Eq for HeapFile {}
impl PartialEq for HeapFile {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
    }
}
impl PartialOrd for HeapFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse：最小堆（堆顶是最小的元素）
        other.size.cmp(&self.size)
    }
}

// ─── 公开接口 ──────────────────────────────────────────────────────────

/// 扫描整个 NTFS 卷，返回空间使用报告。
///
/// `drive`: 形如 `"C:"` 或 `"C:\\"`。
/// `top_n`: 返回最大的 N 个文件。
///
/// 仅支持 NTFS 卷，需要管理员权限。
pub fn scan_volume(drive: &str, top_n: usize) -> Result<disk_usage::UsageReport, String> {
    // 检查管理员权限
    if !crate::privilege::is_elevated() {
        return Err("需要管理员权限才能扫描 MFT。请以管理员身份运行。".to_string());
    }

    let started = std::time::Instant::now();
    let vol_path = normalize_path(drive)?;
    let handle = open_volume(&vol_path)?;

    let result = scan_volume_inner(handle, top_n);
    unsafe { let _ = CloseHandle(handle); }
    result.map(|mut r| {
        r.elapsed_ms = started.elapsed().as_millis() as u64;
        r
    })
}

// ─── 内部实现 ──────────────────────────────────────────────────────────

fn scan_volume_inner(handle: HANDLE, top_n: usize) -> Result<disk_usage::UsageReport, String> {
    let (bytes_per_sector, mft_record_size, mft_valid_length) = get_volume_info(handle)?;

    if mft_record_size == 0 {
        return Err("MFT 记录大小为 0，无法扫描。".to_string());
    }

    let num_records = (mft_valid_length / mft_record_size as u64) as u64;
    if num_records == 0 {
        return Err("MFT 有效长度为 0，无法扫描。".to_string());
    }

    // 预分配容量
    let mut all_entries: Vec<MftEntry> = Vec::with_capacity((num_records as usize / 4).max(1000));

    for frn in 0..num_records {
        match read_one_record(handle, frn, mft_record_size, bytes_per_sector) {
            Ok(Some(mut ents)) => all_entries.append(&mut ents),
            Ok(None) => {}
            Err(_) => {
                // 单个记录失败不中止，继续扫描
            }
        }
    }

    if all_entries.is_empty() {
        return Err("未能读取任何有效的 MFT 记录。".to_string());
    }

    Ok(aggregate(all_entries, top_n))
}

// ─── 辅助：路径规范化 ─────────────────────────────────────────────────

fn normalize_path(drive: &str) -> Result<String, String> {
    let trimmed = drive.trim().trim_end_matches('\\').trim_end_matches(':');
    if trimmed.len() != 1 || !trimmed.as_bytes()[0].is_ascii_alphabetic() {
        return Err(format!("无效盘符: {}", drive));
    }
    Ok(format!("\\\\.\\{}:", trimmed.to_uppercase()))
}

// ─── 辅助：打开卷 ─────────────────────────────────────────────────────

fn open_volume(path: &str) -> Result<HANDLE, String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0x8000_0000u32, // GENERIC_READ
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None, // lpSecurityAttributes
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None, // hTemplateFile
        )
    };
    match handle {
        Ok(h) => {
            // INVALID_HANDLE_VALUE = -1
            if h.0 == (-1isize) as *mut _ {
                return Err(format!("无法打开卷 {}。请以管理员身份运行本程序。", path));
            }
            Ok(h)
        }
        Err(e) => Err(format!("无法打开卷 {}: {}", path, e)),
    }
}

// ─── 辅助：获取 MFT 元信息 ────────────────────────────────────────────

fn get_volume_info(handle: HANDLE) -> Result<(u32, u32, u64), String> {
    let mut vol_data: NTFS_VOLUME_DATA_BUFFER = unsafe { mem::zeroed() };
    let mut bytes_ret = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_NTFS_VOLUME_DATA,
            None, // lpInBuffer
            0,
            Some(&mut vol_data as *mut _ as *mut core::ffi::c_void),
            mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            Some(&mut bytes_ret),
            None, // lpOverlapped
        )
    };

    if ok.is_err() || bytes_ret < mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32 {
        let err = std::io::Error::last_os_error();
        return Err(format!(
            "FSCTL_GET_NTFS_VOLUME_DATA 失败（可能不是 NTFS 卷）: {}",
            err
        ));
    }

    let bps = vol_data.BytesPerSector as u32;
    let mft_rec_sz = vol_data.BytesPerFileRecordSegment as u32;
    let mft_valid = vol_data.MftValidDataLength as u64;

    if bps == 0 || mft_rec_sz == 0 {
        return Err("MFT 卷数据无效（BytesPerSector 或 BytesPerFileRecordSegment 为 0）。".to_string());
    }

    Ok((bps, mft_rec_sz, mft_valid))
}

// ─── 核心：读取一条 MFT 记录 ──────────────────────────────────────────

fn read_one_record(
    handle: HANDLE,
    frn: u64,
    record_size: u32,
    bytes_per_sector: u32,
) -> Result<Option<Vec<MftEntry>>, String> {
    // 输入缓冲
    let input = NTFS_FILE_RECORD_INPUT_BUFFER {
        FileReferenceNumber: frn as i64,
    };

    // 输出缓冲：头 + 最大记录长度
    let out_buf_size = (12 + record_size as usize) as u32; // 12 = offset of FileRecordBuffer
    let mut out_buf = vec![0u8; out_buf_size as usize];
    let mut bytes_ret = 0u32;

    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_NTFS_FILE_RECORD,
            Some(&input as *const _ as *const core::ffi::c_void),
            mem::size_of::<NTFS_FILE_RECORD_INPUT_BUFFER>() as u32,
            Some(out_buf.as_mut_ptr() as *mut core::ffi::c_void),
            out_buf_size,
            Some(&mut bytes_ret),
            None, // lpOverlapped
        )
    };

    if ok.is_err() {
        // 单个记录读取失败（例如文件系统已释放该记录），静默跳过
        return Ok(None);
    }

    let file_record_length = {
        if bytes_ret < 12 {
            return Ok(None);
        }
        read_u32_le(&out_buf, 8) as usize
    };

    if file_record_length == 0 || file_record_length > record_size as usize {
        return Ok(None);
    }

    // 取记录数据（偏移 12 开始）
    let raw_record = &out_buf[12..12 + file_record_length];
    let mut record_buf = raw_record.to_vec();

    // 应用 USA 修复（如果尚未修复）
    if !ensure_fixup(&mut record_buf, bytes_per_sector as u16) {
        return Ok(None);
    }

    let entries = parse_record(&record_buf, frn)?;
    Ok(Some(entries))
}

// ─── USA 修复 ──────────────────────────────────────────────────────────

/// 检查并应用 Update Sequence Array (USA) 修复。
/// 如果已经修复则不动；返回 false 表示记录损坏。
fn ensure_fixup(record: &mut [u8], bytes_per_sector: u16) -> bool {
    if record.len() < 4 || &record[..4] != b"FILE" {
        return false;
    }

    let usa_off = read_u16_le(record, 4) as usize;
    let usa_cnt = read_u16_le(record, 6) as usize;

    if usa_off == 0 || usa_cnt <= 1 {
        return true; // 无修复或不需要修复
    }

    let sec_sz = bytes_per_sector as usize;
    if sec_sz == 0 || sec_sz > record.len() + 2 {
        return true; // 页面大小异常，跳过修复
    }

    // 检查是否需要修复：末2字节是 USN（未修复）还是原始数据（已修复）
    let sector0_tail = read_u16_le(record, sec_sz - 2);
    let usn = read_u16_le(record, usa_off);

    if sector0_tail != usn {
        // 已修复或记录正常，直接返回
        return true;
    }

    // 需要修复：对每个 sector，用 USA 中的原始值替换末尾 2 字节
    for i in 0..(usa_cnt - 1) {
        let sector_end = (i + 1) * sec_sz - 2;
        if sector_end + 1 >= record.len() {
            break;
        }
        let saved = read_u16_le(record, usa_off + (i + 1) * 2);
        record[sector_end] = saved as u8;
        record[sector_end + 1] = (saved >> 8) as u8;
    }

    true
}

// ─── 解析 MFT 记录 ───────────────────────────────────────────────────

/// 从已修复的 FILE 记录中提取所有 $FILE_NAME 条目。
/// 返回 Vec<MftEntry>（硬链接会对应多个条目）。
fn parse_record(record: &[u8], frn: u64) -> Result<Vec<MftEntry>, String> {
    if record.len() < 24 || &record[..4] != b"FILE" {
        return Ok(vec![]);
    }

    let flags = read_u16_le(record, 22);
    let in_use = (flags & 0x01) != 0;
    let is_dir = (flags & 0x02) != 0;

    if !in_use {
        return Ok(vec![]);
    }

    // 跳过属性列表扩展记录（base record reference 非零）
    let base_ref = read_u48_le(record, 32);
    if base_ref != 0 {
        return Ok(vec![]);
    }

    let attr_off = read_u16_le(record, 20) as usize;
    if attr_off >= record.len() {
        return Ok(vec![]);
    }

    // 从记录中收集：文件大小、各 $FILE_NAME 条目
    let mut file_size: Option<u64> = None;
    let mut file_names: Vec<(u64, String, u64)> = Vec::new(); // (parent_frn, name, modified_secs)

    let mut pos = attr_off;
    loop {
        if pos + 4 > record.len() {
            break;
        }
        let attr_type = read_u32_le(record, pos);
        let attr_len = read_u32_le(record, pos + 4);

        // 结尾标记
        if attr_type == 0xFFFF_FFFF || attr_len == 0 {
            break;
        }
        if attr_len as usize > record.len() - pos || attr_len < 8 {
            break;
        }

        let non_resident = record[pos + 8] != 0;
        let name_len = record[pos + 9] as usize;

        // 根据属性类型处理
        match attr_type {
            0x30 => {
                // $FILE_NAME（总是 resident）
                if !non_resident && !is_dir {
                    // 对于目录也读，但目录名也要记录以便建树
                }
                if !non_resident {
                    let value_off = read_u16_le(record, pos + 20) as usize;
                    let value_len = read_u32_le(record, pos + 16) as usize;
                    let val_start = pos + value_off;

                    if val_start + 66 <= record.len() && val_start + value_len <= record.len() {
                        // 父目录参考号（低 48 位）
                        let parent_frn = read_u48_le(record, val_start);
                        // 修改时间（偏移 16）
                        let modified_ft = read_u64_le(record, val_start + 16);
                        let modified_secs = filetime_to_unix(modified_ft);
                        // 文件名长度（字符数），偏移 64
                        let name_len_chars = record[val_start + 64] as usize;
                        // 文件名空间，偏移 65
                        let _namespace = record[val_start + 65];
                        // 文件名起始（UTF-16LE），偏移 66
                        let name_bytes =
                            &record[val_start + 66..val_start + 66 + name_len_chars * 2];
                        let name_utf16: Vec<u16> = name_bytes
                            .chunks(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        let name = String::from_utf16(&name_utf16).unwrap_or_default();

                        if !name.is_empty() {
                            file_names.push((parent_frn, name, modified_secs));
                        }
                    }
                }
            }
            0x80 => {
                // $DATA（仅当未命名时取大小）
                if name_len == 0 {
                    if non_resident {
                        // 非驻留 $DATA：RealSize 在属性头偏移 48
                        let real_size = read_u64_le(record, pos + 48);
                        file_size = Some(real_size);
                    } else {
                        // 驻留 $DATA：ValueLength 在属性头偏移 16
                        let value_len = read_u32_le(record, pos + 16) as u64;
                        file_size = Some(value_len);
                    }
                }
            }
            _ => {}
        }

        pos += attr_len as usize;
    }

    // 如果没找到 $DATA 属性（空文件或目录），尺寸为 0
    let size = file_size.unwrap_or(0);
    let name_list = if file_names.is_empty() {
        // 退回到使用 FRN 做路径
        vec![(0u64, format!("[FRN-{}]", frn), 0u64)]
    } else {
        file_names
    };

    Ok(name_list
        .into_iter()
        .map(|(parent, name, mtime)| MftEntry {
            frn,
            parent_frn: parent,
            name,
            size,
            is_dir,
            modified_secs: mtime,
        })
        .collect())
}

// ─── 聚合 ─────────────────────────────────────────────────────────────

fn aggregate(entries: Vec<MftEntry>, top_n: usize) -> disk_usage::UsageReport {
    // 1. 建立 FRN → 条目索引（同 FRN 只取第一个）
    let mut entry_map: HashMap<u64, MftEntry> = HashMap::new();
    for e in entries {
        entry_map.entry(e.frn).or_insert(e);
    }

    let root_frn = 5u64; // NTFS 根目录固定为记录 5

    // 2. 构建父子关系
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for (&frn, e) in &entry_map {
        if frn != root_frn && e.parent_frn != 0 {
            children.entry(e.parent_frn).or_default().push(frn);
        }
    }

    // 3. BFS 分配路径（从根目录开始）
    let mut paths: HashMap<u64, String> = HashMap::new();
    let mut queue = VecDeque::new();
    paths.insert(root_frn, String::new());
    queue.push_back(root_frn);

    while let Some(current) = queue.pop_front() {
        let cur_path = paths[&current].clone();
        if let Some(child_frns) = children.get(&current) {
            for &child_frn in child_frns {
                if paths.contains_key(&child_frn) {
                    continue;
                }
                if let Some(e) = entry_map.get(&child_frn) {
                    let child_path = if cur_path.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{}\\{}", cur_path, e.name)
                    };
                    paths.insert(child_frn, child_path);
                    queue.push_back(child_frn);
                }
            }
        }
    }

    // 4. 拓扑排序（BFS 序反转 → 叶节点优先）
    let mut order: Vec<u64> = Vec::new();
    let mut bfs_q = VecDeque::new();
    bfs_q.push_back(root_frn);
    while let Some(cur) = bfs_q.pop_front() {
        order.push(cur);
        if let Some(child_frns) = children.get(&cur) {
            for &cf in child_frns {
                bfs_q.push_back(cf);
            }
        }
    }
    order.reverse(); // 叶节点优先

    // 5. 自底向上累加
    let mut dir_sizes: HashMap<u64, u64> = HashMap::new();
    let mut dir_file_counts: HashMap<u64, u64> = HashMap::new();
    let mut scanned = 0u64;
    let mut heap: BinaryHeap<HeapFile> = BinaryHeap::with_capacity(top_n + 1);

    for &current_frn in &order {
        if let Some(e) = entry_map.get(&current_frn) {
            if e.is_dir {
                // 目录：累加子目录和直接子文件
                let mut total_sz = 0u64;
                let mut total_fc = 0u64;
                if let Some(child_frns) = children.get(&current_frn) {
                    for &cf in child_frns {
                        if let Some(child_entry) = entry_map.get(&cf) {
                            if child_entry.is_dir {
                                // 子目录：取其已累计的大小
                                total_sz += dir_sizes.get(&cf).copied().unwrap_or(0);
                                total_fc += dir_file_counts.get(&cf).copied().unwrap_or(0);
                            } else {
                                // 直接子文件
                                total_sz += child_entry.size;
                                total_fc += 1;
                            }
                        } else {
                            total_sz += dir_sizes.get(&cf).copied().unwrap_or(0);
                            total_fc += dir_file_counts.get(&cf).copied().unwrap_or(0);
                        }
                    }
                }
                dir_sizes.insert(current_frn, total_sz);
                dir_file_counts.insert(current_frn, total_fc);
            } else {
                // 文件
                scanned += 1;
                let sz = e.size;
                if sz > 0 {
                    let path = paths.get(&current_frn).cloned().unwrap_or_default();
                    heap.push(HeapFile {
                        size: sz,
                        path,
                        modified_secs: e.modified_secs,
                    });
                    if heap.len() > top_n {
                        heap.pop(); // 移除最小元素，保持堆大小 = top_n
                    }
                }
            }
        }
    }

    // 6. 组装报告
    let mut large_files: Vec<disk_usage::LargeFile> = Vec::with_capacity(heap.len());
    while let Some(hf) = heap.pop() {
        large_files.push(disk_usage::LargeFile {
            path: hf.path,
            size: hf.size,
            modified_secs: hf.modified_secs,
        });
    }
    large_files.reverse(); // 最大在前

    let mut dirs: Vec<disk_usage::DirSummary> = Vec::new();
    for (&frn, &total_size) in &dir_sizes {
        if frn == root_frn {
            continue;
        }
        if let Some(path) = paths.get(&frn) {
            if !path.is_empty() {
                dirs.push(disk_usage::DirSummary {
                    path: path.clone(),
                    total_size,
                    file_count: *dir_file_counts.get(&frn).unwrap_or(&0),
                });
            }
        }
    }
    dirs.sort_by(|a, b| b.total_size.cmp(&a.total_size));

    disk_usage::UsageReport {
        large_files,
        dirs,
        scanned,
        errors: 0,
        source: "mft".into(),
        elapsed_ms: 0,
    }
}

// ─── 单元测试 ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_u16_le() {
        let buf = [0x34, 0x12];
        assert_eq!(read_u16_le(&buf, 0), 0x1234);
    }

    #[test]
    fn test_read_u32_le() {
        let buf = [0x78, 0x56, 0x34, 0x12];
        assert_eq!(read_u32_le(&buf, 0), 0x12345678);
    }

    #[test]
    fn test_read_u64_le() {
        let buf = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        assert_eq!(read_u64_le(&buf, 0), 0x0706050403020100);
    }

    #[test]
    fn test_read_u48_le() {
        let buf = [0x2a, 0x00, 0x00, 0x00, 0x00, 0x01];
        // 0x2a | (0x01 << 40) = 42 + 1099511627776 = 1099511627818
        assert_eq!(read_u48_le(&buf, 0), 1099511627818);
    }

    #[test]
    fn test_filetime_to_unix_zero() {
        assert_eq!(filetime_to_unix(0), 0);
    }

    #[test]
    fn test_filetime_to_unix_known() {
        // Windows FILETIME for 2023-01-15 10:30:00 UTC (approx 1673778600)
        // FILETIME = (unix_epoch_windows_ticks + unix_seconds * 10_000_000)
        let unix_epoch_ft: u64 = 116444736000000000;
        let test_unix = 1673778600u64;
        let ft = unix_epoch_ft + test_unix * 10_000_000;
        assert_eq!(filetime_to_unix(ft), test_unix);
    }

    /// 构造一个模拟的 MFT FILE 记录来测试解析器
    /// 使用非驻留 $DATA 属性（RealSize 在属性头偏移 48），避免大缓冲
    fn make_mock_file_record(is_dir: bool, size: u64, parent_frn: u64, name: &str) -> Vec<u8> {
        let rec_size = 1024;
        let mut buf = vec![0u8; rec_size];

        // "FILE" 签名
        buf[..4].copy_from_slice(b"FILE");

        // USA 偏移 / 计数 (无实际修复, 但需要合法值)
        let usa_off = 48u16;
        let usa_cnt = 2u16;
        buf[4..6].copy_from_slice(&usa_off.to_le_bytes());
        buf[6..8].copy_from_slice(&usa_cnt.to_le_bytes());

        // 标志: in_use + is_dir
        let flags: u16 = 0x01 | if is_dir { 0x02 } else { 0 };
        buf[22..24].copy_from_slice(&flags.to_le_bytes());

        // 属性偏移 = 56 (留出 USA 空间)
        let attr_off: u16 = 56;
        buf[20..22].copy_from_slice(&attr_off.to_le_bytes());

        // 写入 USA 占位 (usa_off = 48, 2 entries, 4 bytes)
        buf[48..52].copy_from_slice(&[0u8; 4]);

        // 第一个属性: $FILE_NAME (type=0x30, resident)
        let name_utf16: Vec<u16> = name.encode_utf16().collect();
        let name_len_chars = name_utf16.len();
        let file_name_value_len = 66 + name_len_chars * 2; // 固定头 + 文件名
        let file_name_attr_len = 24 + file_name_value_len; // resident header + value

        let mut pos = attr_off as usize;

        // Resident $FILE_NAME attribute
        buf[pos..pos + 4].copy_from_slice(&0x30u32.to_le_bytes()); // type
        buf[pos + 4..pos + 8].copy_from_slice(&(file_name_attr_len as u32).to_le_bytes()); // length
        buf[pos + 8] = 0; // resident
        buf[pos + 9] = 0; // name length (unnamed)
        buf[pos + 10..pos + 12].copy_from_slice(&0u16.to_le_bytes()); // name offset (no name)
        buf[pos + 12..pos + 14].copy_from_slice(&0u16.to_le_bytes()); // flags
        buf[pos + 14..pos + 16].copy_from_slice(&1u16.to_le_bytes()); // attr ID
        buf[pos + 16..pos + 20].copy_from_slice(&(file_name_value_len as u32).to_le_bytes()); // value length
        buf[pos + 20..pos + 22].copy_from_slice(&24u16.to_le_bytes()); // value offset = 24
        buf[pos + 22] = 0; // indexed flag
        buf[pos + 23] = 0; // padding

        // Value starts at pos + 24
        let val_start = pos + 24;

        // Parent FRN
        buf[val_start..val_start + 8].copy_from_slice(&parent_frn.to_le_bytes());
        // Created time (0)
        let test_ft: u64 = 116444736000000000 + 1673778600 * 10_000_000; // 2023-01-15
        buf[val_start + 8..val_start + 16].copy_from_slice(&0u64.to_le_bytes()); // created
        buf[val_start + 16..val_start + 24].copy_from_slice(&test_ft.to_le_bytes()); // modified
        buf[val_start + 24..val_start + 32].copy_from_slice(&0u64.to_le_bytes()); // mft changed
        buf[val_start + 32..val_start + 40].copy_from_slice(&0u64.to_le_bytes()); // accessed
        buf[val_start + 40..val_start + 48].copy_from_slice(&0u64.to_le_bytes()); // allocated size
        buf[val_start + 48..val_start + 56].copy_from_slice(&size.to_le_bytes()); // real size
        buf[val_start + 56..val_start + 60].copy_from_slice(&0u32.to_le_bytes()); // flags
        buf[val_start + 60..val_start + 64].copy_from_slice(&0u32.to_le_bytes()); // reserved
        buf[val_start + 64] = name_len_chars as u8; // name length (chars)
        buf[val_start + 65] = 1; // namespace = Win32
        // Write UTF-16 filename
        for (i, &c) in name_utf16.iter().enumerate() {
            let off = val_start + 66 + i * 2;
            buf[off..off + 2].copy_from_slice(&c.to_le_bytes());
        }

        pos += file_name_attr_len;

        // ---- $DATA (if not a directory) ----
        // Use non-resident $DATA so we can set RealSize without a huge value buffer
        if !is_dir {
            // Non-resident header: 64 bytes
            let non_res_len = 64u32;
            buf[pos..pos + 4].copy_from_slice(&0x80u32.to_le_bytes()); // type = $DATA
            buf[pos + 4..pos + 8].copy_from_slice(&non_res_len.to_le_bytes()); // length
            buf[pos + 8] = 1; // non-resident
            buf[pos + 9] = 0; // name length (unnamed)
            buf[pos + 10..pos + 12].copy_from_slice(&0u16.to_le_bytes()); // name offset
            buf[pos + 12..pos + 14].copy_from_slice(&0u16.to_le_bytes()); // flags
            buf[pos + 14..pos + 16].copy_from_slice(&2u16.to_le_bytes()); // attr ID
            buf[pos + 16..pos + 24].copy_from_slice(&0u64.to_le_bytes()); // lowest VCN = 0
            buf[pos + 24..pos + 32].copy_from_slice(&0u64.to_le_bytes()); // highest VCN = 0
            buf[pos + 32..pos + 34].copy_from_slice(&64u16.to_le_bytes()); // mapping pairs offset (past header)
            buf[pos + 34] = 0; // compression unit
            // padding 5 bytes at 35..40
            buf[pos + 40..pos + 48].copy_from_slice(&size.to_le_bytes()); // allocated size
            buf[pos + 48..pos + 56].copy_from_slice(&size.to_le_bytes()); // real size
            buf[pos + 56..pos + 64].copy_from_slice(&size.to_le_bytes()); // valid data length
            pos += 64;
        }

        // 终止属性标记
        if pos + 8 <= rec_size {
            buf[pos..pos + 4].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
            buf[pos + 4..pos + 8].copy_from_slice(&0u32.to_le_bytes());
        }

        buf
    }

    #[test]
    fn test_parse_file_record_normal_file() {
        let parent = 5u64; // root
        let name = "test.txt";
        let size = 12345u64;
        let record = make_mock_file_record(false, size, parent, name);
        let entries = parse_record(&record, 100).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].frn, 100);
        assert_eq!(entries[0].parent_frn, parent);
        assert_eq!(entries[0].name, name);
        assert_eq!(entries[0].size, size);
        assert!(!entries[0].is_dir);
        assert!(entries[0].modified_secs > 0);
    }

    #[test]
    fn test_parse_file_record_directory() {
        let parent = 5u64;
        let name = "subdir";
        let record = make_mock_file_record(true, 0, parent, name);
        let entries = parse_record(&record, 200).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].frn, 200);
        assert_eq!(entries[0].name, name);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].size, 0);
    }

    #[test]
    fn test_parse_file_record_not_in_use() {
        let mut record = make_mock_file_record(false, 100, 5, "gone.txt");
        // Clear the in-use flag
        let flags = read_u16_le(&record, 22);
        let new_flags = flags & !0x01;
        record[22..24].copy_from_slice(&new_flags.to_le_bytes());
        let entries = parse_record(&record, 300).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_parse_file_record_extension_record() {
        let mut record = make_mock_file_record(false, 100, 5, "ext.txt");
        // Set base record reference to non-zero (extension record)
        record[32..40].copy_from_slice(&42u64.to_le_bytes());
        let entries = parse_record(&record, 400).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_aggregate_simple_tree() {
        // 构造树: root(5) -> dir1(10) -> file1(20)
        //                    -> file2(30)
        let entries = vec![
            MftEntry { frn: 5, parent_frn: 0, name: "".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 10, parent_frn: 5, name: "subdir".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 20, parent_frn: 10, name: "file1.txt".to_string(), size: 1000, is_dir: false, modified_secs: 100 },
            MftEntry { frn: 30, parent_frn: 5, name: "file2.txt".to_string(), size: 500, is_dir: false, modified_secs: 200 },
        ];

        let report = aggregate(entries, 5);

        // 检查文件列表
        assert_eq!(report.large_files.len(), 2);
        assert_eq!(report.large_files[0].size, 1000);
        assert_eq!(report.large_files[0].path, "subdir\\file1.txt");
        assert_eq!(report.large_files[1].size, 500);
        assert_eq!(report.large_files[1].path, "file2.txt");

        // 检查目录
        assert_eq!(report.dirs.len(), 1);
        assert_eq!(report.dirs[0].path, "subdir");
        assert_eq!(report.dirs[0].total_size, 1000);
        assert_eq!(report.dirs[0].file_count, 1);
        assert_eq!(report.scanned, 2);
    }

    #[test]
    fn test_aggregate_top_n_heap() {
        let mut entries = vec![
            MftEntry { frn: 5, parent_frn: 0, name: "".to_string(), size: 0, is_dir: true, modified_secs: 0 },
        ];
        // 添加 10 个文件到根目录
        for i in 0..10 {
            entries.push(MftEntry {
                frn: 100 + i,
                parent_frn: 5,
                name: format!("file{}.dat", i),
                size: (i as u64 + 1) * 100,
                is_dir: false,
                modified_secs: 0,
            });
        }

        let report = aggregate(entries, 3);
        assert_eq!(report.large_files.len(), 3);
        // 最大三个: 1000, 900, 800
        assert_eq!(report.large_files[0].size, 1000);
        assert_eq!(report.large_files[1].size, 900);
        assert_eq!(report.large_files[2].size, 800);
    }

    #[test]
    fn test_aggregate_orphaned_file() {
        // 只有一个文件，其父 FRN=5（根目录），但根目录不在 entry_map 中
        // BFS 仍会从 FRN 5 开始，找到该文件（因为它被注册在 children[5] 中）
        let entries = vec![
            MftEntry { frn: 100, parent_frn: 5, name: "orphan.txt".to_string(), size: 100, is_dir: false, modified_secs: 0 },
        ];
        let report = aggregate(entries, 10);
        // 该文件仍能被发现并分配路径
        assert_eq!(report.large_files.len(), 1);
        assert_eq!(report.large_files[0].size, 100);
        assert_eq!(report.large_files[0].path, "orphan.txt");
    }

    #[test]
    fn test_aggregate_nested_dirs() {
        // root(5) -> a(10) -> b(20) -> file(30)
        //                    -> c(25) -> file2(35)
        //         -> d(40) -> file3(45)
        let entries = vec![
            MftEntry { frn: 5, parent_frn: 0, name: "".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 10, parent_frn: 5, name: "a".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 20, parent_frn: 10, name: "b".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 30, parent_frn: 20, name: "file.txt".to_string(), size: 500, is_dir: false, modified_secs: 0 },
            MftEntry { frn: 25, parent_frn: 10, name: "c".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 35, parent_frn: 25, name: "file2.txt".to_string(), size: 300, is_dir: false, modified_secs: 0 },
            MftEntry { frn: 40, parent_frn: 5, name: "d".to_string(), size: 0, is_dir: true, modified_secs: 0 },
            MftEntry { frn: 45, parent_frn: 40, name: "file3.txt".to_string(), size: 200, is_dir: false, modified_secs: 0 },
        ];

        let report = aggregate(entries, 5);
        assert_eq!(report.scanned, 3);

        // dir b = 500, dir c = 300, dir a = 800, dir d = 200
        let mut dir_map: HashMap<&str, (u64, u64)> = HashMap::new();
        for d in &report.dirs {
            dir_map.insert(d.path.as_str(), (d.total_size, d.file_count));
        }

        assert_eq!(dir_map.get("a"), Some(&(800, 2))); // 500 + 300, 2 files
        assert_eq!(dir_map.get("a\\b"), Some(&(500, 1)));
        assert_eq!(dir_map.get("a\\c"), Some(&(300, 1)));
        assert_eq!(dir_map.get("d"), Some(&(200, 1)));
    }

    #[test]
    fn test_ensure_fixup() {
        // Mock 一个不需要修复的小记录 (sec_sz > record.len)
        let mut buf = vec![0u8; 256];
        buf[..4].copy_from_slice(b"FILE");
        buf[4..6].copy_from_slice(&48u16.to_le_bytes()); // usa_off
        buf[6..8].copy_from_slice(&2u16.to_le_bytes()); // usa_cnt
        assert!(ensure_fixup(&mut buf, 4096)); // sec_sz > len, no-op
        assert_eq!(&buf[..4], b"FILE");
    }
}
