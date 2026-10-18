//! 历史趋势持久化：定长追加记录 + 段轮转 + 1 分钟 rollup + 保留/上限淘汰 + 读盘抽稀。
//!
//! 设计依据：`docs/research/2026-10-18-history-persistence-analysis.md` §3–§5；
//! 实测数字：`docs/research/evidence/2026-10-18-storage-and-adapter-measurements.md`。
//!
//! * 目录：安装版 `%LOCALAPPDATA%\WinTop\history\`；便携版（exe 同级存在 `wintop.portable`
//!   标记文件）用 `<exe 同级>\WinTop-data\history\`。
//! * 记录：RAW 64 B / ROLL 80 B，全部小端、逐字段自然对齐、逐条 CRC32（文档 §4.4 的 v2 编码）。
//! * 写入：每 15 秒批量追加（**不**逐条 fsync），每 60 秒与退出时 `sync_all`。
//! * 保留：原始 1 秒 × 7 天 → 1 分钟聚合 × 90 天；硬上限 200 MiB，段粒度最旧优先淘汰。
//! * 降级：目录不可建 / 拿不到 `history.lock`（双实例）/ 磁盘将满 / 写入失败 → 一律退回
//!   "纯内存"（= 本功能落地前的行为）。采集线程**永远不会**因为磁盘问题崩溃或阻塞：
//!   所有磁盘操作都在本模块自己的写线程里，`history::push()` 语义不变。
//!
//! 未持久化的部分（与文档一致，不是遗漏）：每适配器 GPU 明细（`GpuHistSample.adapters`）
//! 计划走"每 LUID 一个旁路段文件"（文档 §4.4），本阶段按 64 B 主记录实现，因此读回时
//! `adapters` 为空、`luid` 由 `u64` 还原。实时（内存环）仍有完整明细。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use tauri::AppHandle;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use crate::collector::MetricsSnapshot;

// ---------------------------------------------------------------------------
// 常量与配置
// ---------------------------------------------------------------------------

/// RAW 记录（1 秒）字节数。
pub const RAW_BYTES: usize = 64;
/// ROLL 记录（1 分钟聚合）字节数。
pub const ROLL_BYTES: usize = 80;
/// 布局版本，写在文件名前缀里。
const LAYOUT: &str = "v2";
/// 原始层保留 7 天。
const RAW_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// 1 分钟聚合层保留 90 天。
const ROLL_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;
/// 目录字节硬上限 200 MiB（文档 §5.4）。
pub const CAP_BYTES: u64 = 200 * 1024 * 1024;
/// 卷可用空间低于此值时暂停持久化（文档 §5.4 第二道保护）。
const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
/// 批量落盘间隔。
const FLUSH_EVERY: Duration = Duration::from_secs(15);
/// 强制落盘（fsync）间隔。
const SYNC_EVERY: Duration = Duration::from_secs(60);
/// 段扫描 + 保留/上限检查间隔。
const SCAN_EVERY: Duration = Duration::from_secs(300);
/// 降级后的重试间隔（不热重试）。
const RETRY_EVERY: Duration = Duration::from_secs(600);
/// 写线程 tick（便于及时响应退出信号，且 CPU 可忽略）。
const TICK: Duration = Duration::from_millis(200);
/// 连续写失败多少次后退避。
const MAX_CONSECUTIVE_FAILURES: u32 = 5;
/// 1 分钟聚合的桶宽。
const ROLL_BUCKET_MS: i64 = 60_000;
/// `get_history_span` 的 max_points 硬上限（防止调用方传 10^6 打爆内存）。
const MAX_POINTS_LIMIT: usize = 5000;

const RAW_SPAN_MS: i64 = 3_600_000;
const ROLL_SPAN_MS: i64 = 86_400_000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 对外状态
// ---------------------------------------------------------------------------

/// 持久化状态（冻结接口：`get_history_status` 的返回类型）。
#[derive(Serialize, Clone, Default)]
pub struct HistoryStatus {
    /// 功能是否开启（当前恒为 true；将来接入设置开关）。
    pub enabled: bool,
    /// 是否真的在落盘（false = 已降级为纯内存）。
    pub writable: bool,
    /// 落盘目录。
    pub dir: String,
    /// 目录内历史文件总字节数。
    pub bytes: u64,
    /// 最旧可用记录的 `ts_ms`（0 = 无数据）。
    pub oldest_ms: i64,
    /// 最新已落盘记录的 `ts_ms`（0 = 无数据）。
    pub newest_ms: i64,
    /// 不可用时的人话原因（可用时为空串）。
    pub reason: String,
    /// 硬上限。
    pub cap_bytes: u64,
    /// 批量落盘周期（秒）。**由后端提供**：前端状态行原样显示，
    /// 若前端硬编码这个数字，后端调整 `FLUSH_EVERY` 时界面就会静默说谎。
    pub flush_secs: u64,
    /// 段文件数量（诊断用）。
    pub raw_segments: u32,
    pub roll_segments: u32,
}

static STATUS: LazyLock<Mutex<HistoryStatus>> = LazyLock::new(|| {
    Mutex::new(HistoryStatus {
        enabled: true,
        writable: false,
        dir: resolve_dir().to_string_lossy().to_string(),
        reason: "历史持久化尚未启动".to_string(),
        cap_bytes: CAP_BYTES,
        flush_secs: FLUSH_EVERY.as_secs(),
        ..Default::default()
    })
});

/// 读取当前状态（廉价，只加一次锁）。
pub fn status() -> HistoryStatus {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut HistoryStatus)) {
    if let Ok(mut s) = STATUS.lock() {
        f(&mut s);
    }
}

// ---------------------------------------------------------------------------
// 目录解析（安装版 / 便携版）
// ---------------------------------------------------------------------------

/// 便携版标记文件名：与 exe 同级存在该文件即视为便携版。
const PORTABLE_MARKER: &str = "wintop.portable";

/// 纯函数：解析落盘目录。不创建任何东西，可在任何线程任意调用。
fn resolve_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join(PORTABLE_MARKER).exists() {
                // 便携版：自包含，绝不写宿主机 %LOCALAPPDATA%
                return dir.join("WinTop-data").join("history");
            }
        }
    }
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default())
        .join("WinTop")
        .join("history")
}

fn free_bytes(dir: &Path) -> Option<u64> {
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free: u64 = 0;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR::from_raw(wide.as_ptr()), Some(&mut free), None, None).ok()?;
    }
    Some(free)
}

// ---------------------------------------------------------------------------
// CRC32（IEEE 802.3 / zlib 多项式；自实现，不引入依赖）
// ---------------------------------------------------------------------------

static CRC_TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
});

fn crc32(data: &[u8]) -> u32 {
    let table = &*CRC_TABLE;
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ---------------------------------------------------------------------------
// 记录编解码（文档 §4.4 的 v2 布局）
// ---------------------------------------------------------------------------

/// GPU 标量（主记录里只存这三个；每适配器明细未持久化，见模块头注释）。
#[derive(Clone, Copy, Default, PartialEq)]
struct GpuScalars {
    available: bool,
    pct: f32,
    luid: u64,
}

/// RAW 64 B：ts_ms u64 | 8×f32 | gpu_luid u64 | mem_load u32 | gpu_flags u32 | gpu_pct f32 | crc32 u32
#[derive(Clone, Copy, Default)]
struct RawRec {
    ts_ms: i64,
    /// cpu, mem_used_gb, mem_total_gb, mem_page_total_gb, mem_page_used_gb,
    /// disk_read_bps, disk_write_bps, net_total_bps
    v: [f32; 8],
    mem_load: u32,
    gpu: GpuScalars,
}

fn encode_raw(r: &RawRec, out: &mut [u8; RAW_BYTES]) {
    out.fill(0);
    out[0..8].copy_from_slice(&r.ts_ms.to_le_bytes());
    for (i, val) in r.v.iter().enumerate() {
        let o = 8 + i * 4;
        out[o..o + 4].copy_from_slice(&val.to_le_bytes());
    }
    out[40..48].copy_from_slice(&r.gpu.luid.to_le_bytes());
    out[48..52].copy_from_slice(&r.mem_load.to_le_bytes());
    out[52..56].copy_from_slice(&(u32::from(r.gpu.available)).to_le_bytes());
    out[56..60].copy_from_slice(&r.gpu.pct.to_le_bytes());
    let c = crc32(&out[..60]);
    out[60..64].copy_from_slice(&c.to_le_bytes());
}

fn decode_raw(b: &[u8]) -> Option<RawRec> {
    if b.len() < RAW_BYTES {
        return None;
    }
    let body = &b[..60];
    let want = u32::from_le_bytes(b[60..64].try_into().ok()?);
    if crc32(body) != want {
        return None;
    }
    let mut v = [0f32; 8];
    for (i, slot) in v.iter_mut().enumerate() {
        let o = 8 + i * 4;
        *slot = f32::from_le_bytes(body[o..o + 4].try_into().ok()?);
    }
    Some(RawRec {
        ts_ms: i64::from_le_bytes(body[0..8].try_into().ok()?),
        v,
        gpu: GpuScalars {
            available: u32::from_le_bytes(body[52..56].try_into().ok()?) & 1 != 0,
            pct: f32::from_le_bytes(body[56..60].try_into().ok()?),
            luid: u64::from_le_bytes(body[40..48].try_into().ok()?),
        },
        mem_load: u32::from_le_bytes(body[48..52].try_into().ok()?),
    })
}

/// ROLL 80 B：ts_ms | 8×f32(均值) | gpu_luid | cpu_max | gpu_pct_avg | gpu_pct_max |
/// samples u16 | pad u16 | mem_load_avg u32 | gpu_flags u32 | reserved u32 | crc32 u32
#[derive(Clone, Copy, Default)]
struct RollRec {
    ts_ms: i64,
    v: [f32; 8],
    gpu_luid: u64,
    cpu_max: f32,
    gpu_pct_avg: f32,
    gpu_pct_max: f32,
    samples: u16,
    mem_load_avg: u32,
    gpu_available: bool,
}

fn encode_roll(r: &RollRec, out: &mut [u8; ROLL_BYTES]) {
    out.fill(0);
    out[0..8].copy_from_slice(&r.ts_ms.to_le_bytes());
    for (i, val) in r.v.iter().enumerate() {
        let o = 8 + i * 4;
        out[o..o + 4].copy_from_slice(&val.to_le_bytes());
    }
    out[40..48].copy_from_slice(&r.gpu_luid.to_le_bytes());
    out[48..52].copy_from_slice(&r.cpu_max.to_le_bytes());
    out[52..56].copy_from_slice(&r.gpu_pct_avg.to_le_bytes());
    out[56..60].copy_from_slice(&r.gpu_pct_max.to_le_bytes());
    out[60..62].copy_from_slice(&r.samples.to_le_bytes());
    // 62..64 保留
    out[64..68].copy_from_slice(&r.mem_load_avg.to_le_bytes());
    out[68..72].copy_from_slice(&(u32::from(r.gpu_available)).to_le_bytes());
    // 72..76 保留
    let c = crc32(&out[..76]);
    out[76..80].copy_from_slice(&c.to_le_bytes());
}

fn decode_roll(b: &[u8]) -> Option<RollRec> {
    if b.len() < ROLL_BYTES {
        return None;
    }
    let body = &b[..76];
    let want = u32::from_le_bytes(b[76..80].try_into().ok()?);
    if crc32(body) != want {
        return None;
    }
    let mut v = [0f32; 8];
    for (i, slot) in v.iter_mut().enumerate() {
        let o = 8 + i * 4;
        *slot = f32::from_le_bytes(body[o..o + 4].try_into().ok()?);
    }
    Some(RollRec {
        ts_ms: i64::from_le_bytes(body[0..8].try_into().ok()?),
        v,
        gpu_luid: u64::from_le_bytes(body[40..48].try_into().ok()?),
        cpu_max: f32::from_le_bytes(body[48..52].try_into().ok()?),
        gpu_pct_avg: f32::from_le_bytes(body[52..56].try_into().ok()?),
        gpu_pct_max: f32::from_le_bytes(body[56..60].try_into().ok()?),
        samples: u16::from_le_bytes(body[60..62].try_into().ok()?),
        mem_load_avg: u32::from_le_bytes(body[64..68].try_into().ok()?),
        gpu_available: u32::from_le_bytes(body[68..72].try_into().ok()?) & 1 != 0,
    })
}

// ---------------------------------------------------------------------------
// MetricsSnapshot <-> 记录
// ---------------------------------------------------------------------------

fn luid_string(v: u64) -> String {
    if v == 0 {
        String::new()
    } else {
        format!("0x{:08X}:0x{:08X}", (v >> 32) as u32, v as u32)
    }
}

fn parse_luid(s: &str) -> u64 {
    let mut it = s.split(':');
    let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
    match (it.next().and_then(hex), it.next().and_then(hex)) {
        (Some(hi), Some(lo)) => ((hi as u64) << 32) | lo as u64,
        _ => 0,
    }
}

fn hms(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => t.format("%H:%M:%S").to_string(),
        None => String::new(),
    }
}

fn gpu_scalars(s: &MetricsSnapshot) -> GpuScalars {
    GpuScalars {
        available: s.gpu.available,
        pct: s.gpu.gpu_pct as f32,
        luid: parse_luid(&s.gpu.luid),
    }
}

fn gpu_sample(g: GpuScalars) -> crate::gpu_proc::GpuHistSample {
    crate::gpu_proc::GpuHistSample {
        available: g.available,
        gpu_pct: g.pct as f64,
        luid: luid_string(g.luid),
        // 每适配器明细未持久化（见模块头注释）；实时环里仍有完整明细
        adapters: Vec::new(),
    }
}

fn raw_from_snapshot(s: &MetricsSnapshot) -> RawRec {
    RawRec {
        ts_ms: s.ts_ms,
        v: [
            s.cpu as f32,
            s.mem_used_gb as f32,
            s.mem_total_gb as f32,
            s.mem_page_total_gb as f32,
            s.mem_page_used_gb as f32,
            s.disk_read_bps as f32,
            s.disk_write_bps as f32,
            s.net_total_bps as f32,
        ],
        mem_load: s.mem_load,
        gpu: gpu_scalars(s),
    }
}

fn raw_to_snapshot(r: &RawRec) -> MetricsSnapshot {
    MetricsSnapshot {
        ts: hms(r.ts_ms),
        ts_ms: r.ts_ms,
        cpu: r.v[0] as f64,
        mem_load: r.mem_load,
        mem_used_gb: r.v[1] as f64,
        mem_total_gb: r.v[2] as f64,
        mem_page_total_gb: r.v[3] as f64,
        mem_page_used_gb: r.v[4] as f64,
        disk_read_bps: r.v[5] as f64,
        disk_write_bps: r.v[6] as f64,
        net_total_bps: r.v[7] as f64,
        gpu: gpu_sample(r.gpu),
    }
}

fn roll_to_snapshot(r: &RollRec) -> MetricsSnapshot {
    MetricsSnapshot {
        ts: hms(r.ts_ms),
        ts_ms: r.ts_ms,
        // 聚合点回放用该分钟的**峰值**：长窗口看的是"有没有尖峰"，均值会把尖峰抹平。
        cpu: r.cpu_max as f64,
        mem_load: r.mem_load_avg,
        mem_used_gb: r.v[1] as f64,
        mem_total_gb: r.v[2] as f64,
        mem_page_total_gb: r.v[3] as f64,
        mem_page_used_gb: r.v[4] as f64,
        disk_read_bps: r.v[5] as f64,
        disk_write_bps: r.v[6] as f64,
        net_total_bps: r.v[7] as f64,
        gpu: gpu_sample(GpuScalars {
            available: r.gpu_available,
            pct: r.gpu_pct_max,
            luid: r.gpu_luid,
        }),
    }
}

// ---------------------------------------------------------------------------
// 段文件
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SegKind {
    Raw,
    Roll1m,
}

impl SegKind {
    fn prefix(self) -> &'static str {
        match self {
            SegKind::Raw => "v2-raw-",
            SegKind::Roll1m => "v2-roll1m-",
        }
    }
    fn ext(self) -> &'static str {
        match self {
            SegKind::Raw => ".raw",
            SegKind::Roll1m => ".roll",
        }
    }
    fn record_bytes(self) -> usize {
        match self {
            SegKind::Raw => RAW_BYTES,
            SegKind::Roll1m => ROLL_BYTES,
        }
    }
    fn span_ms(self) -> i64 {
        match self {
            SegKind::Raw => RAW_SPAN_MS,
            SegKind::Roll1m => ROLL_SPAN_MS,
        }
    }
}

fn local_ms(dt: chrono::NaiveDateTime) -> Option<i64> {
    use chrono::TimeZone;
    match chrono::Local.from_local_datetime(&dt) {
        chrono::LocalResult::Single(t) => Some(t.timestamp_millis()),
        chrono::LocalResult::Ambiguous(a, _) => Some(a.timestamp_millis()),
        chrono::LocalResult::None => None,
    }
}

/// 把时刻对齐到它所属的**本地整点**。
///
/// 段文件名是按**本地**时间渲染/解析的（`%Y%m%dT%H`），所以段键必须用**本地对齐**。
/// 之前写入侧用 `div_euclid(RAW_SPAN_MS)`（UTC 对齐）当段键，在 UTC 偏移不是整小时的环境
/// 下会与文件名不一致；对 ROLL 日段更严重：`div_euclid(ROLL_SPAN_MS)` 是 UTC 整日，
/// 而文件名/解析都按本地 → 往返偏移正好等于时区偏移（本机 JST 实测 **−9 小时**）。
/// 后果：读路径用 `seg.start + span` 判断"段与窗口是否相交"时会把边缘段整段跳过，
/// 丢掉该段内的数据（含极值）。修法：所有段键一律用本函数/`local_day_start` 生成。
fn local_hour_start(ms: i64) -> i64 {
    use chrono::Timelike;
    use chrono::TimeZone;
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return ms.div_euclid(RAW_SPAN_MS) * RAW_SPAN_MS;
    };
    t.date_naive()
        .and_hms_opt(t.hour(), 0, 0)
        .and_then(local_ms)
        // DST 跳变导致该本地时刻不存在时，退回 UTC 对齐（只影响 1 小时的分段，不会丢数据）
        .unwrap_or_else(|| ms.div_euclid(RAW_SPAN_MS) * RAW_SPAN_MS)
}

/// 把时刻对齐到它所属的**本地整日**（本地 0 点）。理由见 [`local_hour_start`]。
fn local_day_start(ms: i64) -> i64 {
    use chrono::TimeZone;
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return ms.div_euclid(ROLL_SPAN_MS) * ROLL_SPAN_MS;
    };
    t.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(local_ms)
        .unwrap_or_else(|| ms.div_euclid(ROLL_SPAN_MS) * ROLL_SPAN_MS)
}

fn seg_name(kind: SegKind, start_ms: i64) -> String {
    use chrono::TimeZone;
    let stamp = match chrono::Local.timestamp_millis_opt(start_ms).single() {
        Some(t) if kind == SegKind::Raw => t.format("%Y%m%dT%H").to_string(),
        Some(t) => t.format("%Y%m%d").to_string(),
        None => start_ms.to_string(),
    };
    format!("{}{}{}", kind.prefix(), stamp, kind.ext())
}

/// 解析段文件名 → (类型, 段起点 ms)。不认识的返回 None（**永不删除**）。
fn parse_seg(name: &str) -> Option<(SegKind, i64)> {
    let (kind, rest) = if let Some(r) = name.strip_prefix("v2-raw-") {
        (SegKind::Raw, r.strip_suffix(".raw")?)
    } else if let Some(r) = name.strip_prefix("v2-roll1m-") {
        (SegKind::Roll1m, r.strip_suffix(".roll")?)
    } else {
        return None;
    };
    let ms = match kind {
        SegKind::Raw => {
            // "YYYYMMDDTHH"（11 字节）。chrono 的 NaiveDateTime 解析要求完整时刻，
            // 只有日期 + 小时时必须自己拆开，否则 parse_from_str 一定失败。
            let b = rest.as_bytes();
            if b.len() != 11 || b[8] != b'T' {
                return None;
            }
            let d = chrono::NaiveDate::parse_from_str(rest.get(..8)?, "%Y%m%d").ok()?;
            let h: u32 = rest.get(9..11)?.parse().ok()?;
            local_ms(d.and_hms_opt(h, 0, 0)?)?
        }
        SegKind::Roll1m => {
            let d = chrono::NaiveDate::parse_from_str(rest, "%Y%m%d").ok()?;
            local_ms(d.and_hms_opt(0, 0, 0)?)?
        }
    };
    Some((kind, ms))
}

#[derive(Clone)]
struct SegmentInfo {
    kind: SegKind,
    start_ms: i64,
    path: PathBuf,
    bytes: u64,
}

/// 扫描目录里所有可识别的段，按 (类型, 起点) 升序。
fn list_segments(dir: &Path) -> Vec<SegmentInfo> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((kind, start_ms)) = parse_seg(&name) else {
            continue;
        };
        let Ok(md) = entry.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        out.push(SegmentInfo {
            kind,
            start_ms,
            path: entry.path(),
            bytes: md.len(),
        });
    }
    out.sort_by_key(|s| (s.kind as u8, s.start_ms));
    out
}

/// 纯函数：算出应删除的段（先按保留期，再按硬上限；最旧优先，raw 先于 roll）。
fn plan_eviction(segs: &[SegmentInfo], now: i64, cap_bytes: u64) -> Vec<PathBuf> {
    let mut keep: Vec<&SegmentInfo> = Vec::new();
    let mut doomed: Vec<PathBuf> = Vec::new();
    for s in segs {
        let ttl = match s.kind {
            SegKind::Raw => RAW_RETENTION_MS,
            SegKind::Roll1m => ROLL_RETENTION_MS,
        };
        if now.saturating_sub(s.start_ms) > ttl {
            doomed.push(s.path.clone());
        } else {
            keep.push(s);
        }
    }
    let mut total: u64 = keep.iter().map(|s| s.bytes).sum();
    let mut order = keep.clone();
    // Raw=0 排在 Roll1m=1 之前 → 先删原始段
    order.sort_by_key(|s| (s.kind as u8, s.start_ms));
    for s in order {
        if total <= cap_bytes {
            break;
        }
        total = total.saturating_sub(s.bytes);
        doomed.push(s.path.clone());
    }
    doomed
}

/// 读取段文件末尾最后一条有效记录的 `ts_ms`（用于重启后接着写，不重复）。
fn last_ts_in(path: &Path, rec: usize) -> Option<i64> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len < rec as u64 {
        return None;
    }
    let mut tries = 0;
    let mut end = len;
    while tries < 3 && end >= rec as u64 {
        f.seek(SeekFrom::Start(end - rec as u64)).ok()?;
        let mut buf = vec![0u8; rec];
        f.read_exact(&mut buf).ok()?;
        let ts = match rec {
            RAW_BYTES => decode_raw(&buf).map(|r| r.ts_ms),
            _ => decode_roll(&buf).map(|r| r.ts_ms),
        };
        if let Some(ts) = ts {
            return Some(ts);
        }
        end -= rec as u64;
        tries += 1;
    }
    None
}

/// 读取段文件第一条有效记录的 `ts_ms`。
fn first_ts_in(path: &Path, rec: usize) -> Option<i64> {
    let bytes = fs::read(path).ok()?;
    for chunk in bytes.chunks_exact(rec) {
        let ts = match rec {
            RAW_BYTES => decode_raw(chunk).map(|r| r.ts_ms),
            _ => decode_roll(chunk).map(|r| r.ts_ms),
        };
        if let Some(ts) = ts {
            return Some(ts);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 1 分钟 rollup
// ---------------------------------------------------------------------------

/// 把一段 RAW 字节聚合成 ROLL 字节（按 `ts_ms / 60000` 分桶，桶内跳过坏记录）。
fn rollup_bytes(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: Option<RollAccum> = None;
    for chunk in raw.chunks_exact(RAW_BYTES) {
        let Some(r) = decode_raw(chunk) else { continue };
        let bucket = r.ts_ms.div_euclid(ROLL_BUCKET_MS) * ROLL_BUCKET_MS;
        match &mut acc {
            Some(a) if a.bucket == bucket => a.add(&r),
            Some(_) => {
                if let Some(a) = acc.take() {
                    push_roll(&mut out, &a);
                }
                let mut a = RollAccum::new(bucket);
                a.add(&r);
                acc = Some(a);
            }
            None => {
                let mut a = RollAccum::new(bucket);
                a.add(&r);
                acc = Some(a);
            }
        }
    }
    if let Some(a) = acc {
        push_roll(&mut out, &a);
    }
    out
}

struct RollAccum {
    bucket: i64,
    n: u32,
    sum: [f64; 8],
    mem_sum: f64,
    cpu_max: f32,
    gpu_sum: f64,
    gpu_max: f32,
    gpu_luid: u64,
    gpu_available: bool,
}

impl RollAccum {
    fn new(bucket: i64) -> Self {
        RollAccum {
            bucket,
            n: 0,
            sum: [0.0; 8],
            mem_sum: 0.0,
            cpu_max: 0.0,
            gpu_sum: 0.0,
            gpu_max: 0.0,
            gpu_luid: 0,
            gpu_available: false,
        }
    }
    fn add(&mut self, r: &RawRec) {
        for (s, v) in self.sum.iter_mut().zip(r.v.iter()) {
            *s += *v as f64;
        }
        self.mem_sum += r.mem_load as f64;
        if self.n == 0 || r.v[0] > self.cpu_max {
            self.cpu_max = r.v[0];
        }
        self.gpu_sum += r.gpu.pct as f64;
        if r.gpu.pct > self.gpu_max {
            self.gpu_max = r.gpu.pct;
            self.gpu_luid = r.gpu.luid;
        }
        self.gpu_available |= r.gpu.available;
        self.n += 1;
    }
}

fn push_roll(out: &mut Vec<u8>, a: &RollAccum) {
    if a.n == 0 {
        return;
    }
    let n = a.n as f64;
    let mut rec = RollRec {
        ts_ms: a.bucket,
        gpu_luid: a.gpu_luid,
        cpu_max: a.cpu_max,
        gpu_pct_avg: (a.gpu_sum / n) as f32,
        gpu_pct_max: a.gpu_max,
        samples: a.n.min(u16::MAX as u32) as u16,
        mem_load_avg: (a.mem_sum / n).round() as u32,
        gpu_available: a.gpu_available,
        ..Default::default()
    };
    for (i, s) in a.sum.iter().enumerate() {
        rec.v[i] = (s / n) as f32;
    }
    let mut buf = [0u8; ROLL_BYTES];
    encode_roll(&rec, &mut buf);
    out.extend_from_slice(&buf);
}

// ---------------------------------------------------------------------------
// 抽稀（min/max 包络，尖峰不被均值抹平）
// ---------------------------------------------------------------------------

/// 抽稀器：把任意多的点压到 ≤ `limit` 个，且**保证全局 min 与全局 max 都在结果里**。
///
/// 不变量（`finish()` 之后成立）：
/// * `out.len() <= limit`；
/// * `out` 按 `ts_ms` 升序；
/// * `limit >= 2` 时，`out` 必然包含输入中的全局 cpu 最小值与最大值。
///
/// 实现要点：按"以**首个采样点**为原点的等宽桶"分组，桶数 `buckets = max(1, limit/2)`，
/// 且桶序号**硬截断**到 `buckets-1`，因此非空桶数 ≤ `buckets`；每桶最多输出 min 与 max
/// 两个包络点 ⇒ 输出点数 ≤ `2 × buckets ≤ limit`，**正常路径根本不需要截断**，
/// 全局极值作为各自桶的 min/max 必然被保留。
/// （旧实现按 epoch **绝对对齐**分桶：请求区间最多会多覆盖 1 个桶 ⇒ 可产出 `limit + 2`
/// 个包络点 ⇒ 末尾 `truncate` 从尾部裁掉，正好丢掉窗口末端的全局极值 —— 这就是 30 天
/// 窗口"尖峰可能不出现在图上"的根因。）
struct Downsampler {
    bucket_ms: i64,
    buckets: i64,
    /// 首个采样点的 `ts_ms`，桶以它为原点（避免桶边界随读盘时刻漂移）。
    origin: Option<i64>,
    cur: Option<i64>,
    lo: Option<MetricsSnapshot>,
    hi: Option<MetricsSnapshot>,
    out: Vec<MetricsSnapshot>,
    limit: usize,
}

impl Downsampler {
    fn new(span_ms: i64, limit: usize) -> Self {
        let limit = limit.max(1);
        let buckets = (limit / 2).max(1) as i64;
        let bucket_ms = ((span_ms.max(1000) + buckets - 1) / buckets).max(1000);
        Downsampler {
            bucket_ms,
            buckets,
            origin: None,
            cur: None,
            lo: None,
            hi: None,
            out: Vec::with_capacity(limit.min(1024)),
            limit,
        }
    }

    fn push(&mut self, s: MetricsSnapshot) {
        let origin = *self.origin.get_or_insert(s.ts_ms);
        // 相对原点、并按桶数上限硬截断：保证非空桶数 ≤ buckets
        let rel = (s.ts_ms - origin).max(0);
        let bin = (rel / self.bucket_ms).min(self.buckets - 1);
        if self.cur != Some(bin) {
            self.flush_bucket();
            self.cur = Some(bin);
        }
        let take_lo = self.lo.as_ref().map_or(true, |m| s.cpu < m.cpu);
        let take_hi = self.hi.as_ref().map_or(true, |m| s.cpu > m.cpu);
        if take_lo && take_hi {
            self.hi = Some(s.clone());
            self.lo = Some(s);
        } else if take_lo {
            self.lo = Some(s);
        } else if take_hi {
            self.hi = Some(s);
        }
    }

    fn flush_bucket(&mut self) {
        let Some(lo) = self.lo.take() else {
            self.hi = None;
            return;
        };
        let hi = self.hi.take();
        self.out.push(lo.clone());
        if let Some(h) = hi {
            if h.ts_ms != lo.ts_ms {
                self.out.push(h);
            }
        }
    }

    fn finish(mut self) -> Vec<MetricsSnapshot> {
        self.flush_bucket();
        self.out.sort_by_key(|s| s.ts_ms);
        if self.out.len() > self.limit {
            // 只可能在 limit == 1 时走到这里（buckets=1 ⇒ 最多 2 个包络点）。
            // 兜底仍按"保极值"裁剪，绝不静默丢掉尖峰。
            self.out = truncate_keeping_extremes(self.out, self.limit);
        }
        self.out
    }
}

/// 保极值截断：优先保留全局 **max**（尖峰最该被看到）、再保留全局 min，其余按时间顺序补足。
/// 仅当 `limit == 1`（无法同时容纳两个极值）或将来桶数计算出错时才会被调用。
fn truncate_keeping_extremes(v: Vec<MetricsSnapshot>, limit: usize) -> Vec<MetricsSnapshot> {
    let limit = limit.max(1);
    if v.len() <= limit {
        return v;
    }
    let mut max_i = 0usize;
    let mut min_i = 0usize;
    for (i, s) in v.iter().enumerate() {
        if s.cpu > v[max_i].cpu {
            max_i = i;
        }
        if s.cpu < v[min_i].cpu {
            min_i = i;
        }
    }
    let mut picked: Vec<usize> = Vec::with_capacity(limit);
    picked.push(max_i);
    if picked.len() < limit && min_i != max_i {
        picked.push(min_i);
    }
    for i in 0..v.len() {
        if picked.len() >= limit {
            break;
        }
        if !picked.contains(&i) {
            picked.push(i);
        }
    }
    picked.sort_unstable();
    picked.into_iter().map(|i| v[i].clone()).collect()
}

// ---------------------------------------------------------------------------
// 读盘
// ---------------------------------------------------------------------------

/// 读取最近 `seconds` 秒的历史，抽稀到至多 `max_points` 个点。
///
/// # 数据来源（两层，按时间严格互斥切分，**不重复计数**）
/// * 跨度 ≤ 原始层保留期（7 天）：原始层本身就是完整来源，只读 RAW（1 秒分辨率）。
/// * 跨度 > 7 天：以 **1 分钟聚合层**为准；凡是聚合层覆盖不到的区间，都用**原始层**补齐。
///   聚合层的覆盖区间 `[roll_first, roll_last]` 由实际记录算出，切分为三段：
///   `RAW [from, roll_first-1]` → `ROLL [roll_first, roll_last]` → `RAW [roll_last+1, now]`。
///   三段**互不相交**，因此同一个 `ts_ms` 只可能来自其中一层/一段。
///   两端都要补，原因不同：
///   - **左端**（`from < roll_first`）：聚合层要等第一个小时边界才产生 → 全新安装后的
///     头一小时内，7 天/30 天窗口曾是空图（本函数回退原始层修掉的就是这个）；
///   - **右端**（`roll_last < now`）：聚合层天然落后于「当前这一小时」→ 若不补，
///     长窗口曲线右端会缺最新几十分钟。
///   聚合层完全不存在（全新安装 / 聚合层被删）时，整段窗口都回退到原始层。
///
/// # 保证（前端可依赖的契约）
/// * 返回按 `ts_ms` **升序**；
/// * 长度 **≤ `max_points`**（`max_points == 0` 被夹到 1，硬上限 5000）；
/// * **`max_points >= 2` 时，结果必然包含窗口内的全局 cpu 最小值与最大值** ——
///   抽稀按时间桶保留每桶的 min/max 两个包络点（既不是等距抽样、也不是均值），
///   因此长窗口下的尖峰不会被抹平，也不会因裁剪而消失（详见 [`Downsampler`] 的不变量）；
///   混合两层时这个保证依然成立（两层喂的是同一个抽稀器）；
/// * `max_points == 1` 时只返回一个点，取窗口内 cpu 最大值（尖峰优先）；
/// * 桶数 = `max(1, max_points/2)`，桶宽 = `ceil(跨度/桶数)`；桶以**首个采样点**为原点，
///   所以同样的输入数据在任何时刻读都得到同样的结果（不受读盘时刻影响，测试可确定性复现）；
/// * 段键（文件名里的时间）一律按**本地时间**对齐（`local_hour_start` / `local_day_start`），
///   与 `seg_name`/`parse_seg` 的本地渲染/解析一致 —— 这保证段键往返精确，读路径不会因为
///   时区偏移而整段跳过数据；
/// * 时间参数化：内部实现 [`read_span_at`] 的 `now` 是显式参数，公开入口 [`read_span`] 传
///   `now_ms()`；测试钉死 `now` 后与机器时钟完全无关；
/// * 只读与请求区间有交集的段（段级预筛故意放宽一个段长，宁可多读一段也不漏）；某段读失败
///   （IO/权限）时跳过该段，绝不 panic。
pub fn read_span(seconds: u64, max_points: usize) -> Vec<MetricsSnapshot> {
    read_span_at(&resolve_dir(), seconds, max_points, now_ms())
}

/// 读盘核心实现：**`now` 是显式参数**，不在这里读时钟。
///
/// 这样测试可以钉死一个时刻（`read_span_at(dir, secs, pts, FIXED_NOW)`），结果与机器
/// 时钟完全解耦；生产入口 [`read_span`] 传 `now_ms()`。本模块的读路径曾因为测试依赖
/// 真实时钟、且段键按 UTC 对齐而段名按本地渲染，在跨过本地时区边界后必失败。
fn read_span_at(
    dir: &Path,
    seconds: u64,
    max_points: usize,
    now: i64,
) -> Vec<MetricsSnapshot> {
    let limit = max_points.clamp(1, MAX_POINTS_LIMIT);
    let span = (seconds.max(1) as i64).saturating_mul(1000);
    let from = now.saturating_sub(span);
    let mut ds = Downsampler::new(span, limit);

    if span <= RAW_RETENTION_MS {
        // 短窗口：原始层是完整来源（聚合层分辨率不够，没必要用）
        feed_raw(dir, from, now, &mut ds);
        return ds.finish();
    }

    // 长窗口：聚合层优先 + 原始层补两端空隙。
    // 三段严格切分：RAW[from, roll_first-1] / ROLL[roll_first, roll_last] / RAW[roll_last+1, now]
    // → 互不重叠、不重复计数；喂入顺序天然按时间升序。
    match rollup_coverage(dir, from, now) {
        Some((roll_first, roll_last)) => {
            if roll_first - 1 >= from {
                feed_raw(dir, from, roll_first - 1, &mut ds);
            }
            feed_roll(dir, roll_first, roll_last, &mut ds);
            // 聚合层落后于 now 的部分（当前这一小时还没 rollup）用原始层补上
            if roll_last + 1 <= now {
                feed_raw(dir, roll_last + 1, now, &mut ds);
            }
        }
        None => {
            // 窗口内完全没有聚合数据（全新安装 / 聚合层被删）→ 整段回退原始层
            feed_raw(dir, from, now, &mut ds);
        }
    }
    ds.finish()
}

/// 把 `[from_ms, to_ms]` 内、类型为 `kind` 的记录喂给抽稀器（段按时间升序，天然递增）。
///
/// 段级预筛**故意放宽一个段长**：即使段键（来自文件名）与段内记录的时间戳之间存在
/// 时区/对齐误差，也绝不会整段被静默跳过；精确过滤在下面按每条记录的 `ts_ms` 进行。
fn feed_segments(dir: &Path, kind: SegKind, from_ms: i64, to_ms: i64, ds: &mut Downsampler) {
    if from_ms > to_ms {
        return;
    }
    let rec = kind.record_bytes();
    let pad = kind.span_ms();
    for seg in list_segments(dir) {
        if seg.kind != kind {
            continue;
        }
        if seg.start_ms + 2 * pad < from_ms || seg.start_ms > to_ms + pad {
            continue;
        }
        let Ok(bytes) = fs::read(&seg.path) else {
            continue;
        };
        for chunk in bytes.chunks_exact(rec) {
            let snap = match kind {
                SegKind::Raw => decode_raw(chunk).map(|r| raw_to_snapshot(&r)),
                SegKind::Roll1m => decode_roll(chunk).map(|r| roll_to_snapshot(&r)),
            };
            if let Some(s) = snap {
                if s.ts_ms >= from_ms && s.ts_ms <= to_ms {
                    ds.push(s);
                }
            }
        }
    }
}

fn feed_raw(dir: &Path, from_ms: i64, to_ms: i64, ds: &mut Downsampler) {
    feed_segments(dir, SegKind::Raw, from_ms, to_ms, ds);
}

fn feed_roll(dir: &Path, from_ms: i64, to_ms: i64, ds: &mut Downsampler) {
    feed_segments(dir, SegKind::Roll1m, from_ms, to_ms, ds);
}

/// 聚合层在 `[from_ms, to_ms]` 窗口内的**实际覆盖区间** `(first, last)`（均已夹到窗口内）。
/// `None` = 窗口内完全没有聚合数据。
///
/// 用途：长窗口回退时决定原始层要补哪两段（左端空隙 + 右端「尚未 rollup」的尾部），
/// 见 [`read_span_at`] 的三段切分策略。段级预筛同样放宽一个段长（见 [`feed_segments`] 的说明）。
fn rollup_coverage(dir: &Path, from_ms: i64, to_ms: i64) -> Option<(i64, i64)> {
    let mut segs: Vec<SegmentInfo> = list_segments(dir)
        .into_iter()
        .filter(|s| s.kind == SegKind::Roll1m)
        .filter(|s| {
            let pad = s.kind.span_ms();
            s.start_ms + 2 * pad >= from_ms && s.start_ms <= to_ms + pad
        })
        .collect();
    if segs.is_empty() {
        return None;
    }
    segs.sort_by_key(|s| s.start_ms);
    let first = segs
        .iter()
        .find_map(|s| first_ts_in(&s.path, ROLL_BYTES))?
        .max(from_ms);
    let last = segs
        .iter()
        .rev()
        .find_map(|s| last_ts_in(&s.path, ROLL_BYTES))?
        .min(to_ms);
    if first > last {
        return None;
    }
    Some((first, last))
}

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct Manifest {
    layout: String,
    record_bytes: u32,
    roll_record_bytes: u32,
    /// 字段表（名:偏移:类型），供排障与版本迁移用。
    raw_fields: Vec<String>,
    created_ms: i64,
    last_write_ms: i64,
    /// 已做过 rollup 的小时起点，避免重启后重复聚合。
    rolled_hours: Vec<i64>,
}

impl Manifest {
    fn new() -> Self {
        Manifest {
            layout: LAYOUT.to_string(),
            record_bytes: RAW_BYTES as u32,
            roll_record_bytes: ROLL_BYTES as u32,
            raw_fields: vec![
                "ts_ms:0:u64".into(),
                "cpu:8:f32".into(),
                "mem_used_gb:12:f32".into(),
                "mem_total_gb:16:f32".into(),
                "mem_page_total_gb:20:f32".into(),
                "mem_page_used_gb:24:f32".into(),
                "disk_read_bps:28:f32".into(),
                "disk_write_bps:32:f32".into(),
                "net_total_bps:36:f32".into(),
                "gpu_luid:40:u64".into(),
                "mem_load:48:u32".into(),
                "gpu_flags:52:u32".into(),
                "gpu_pct:56:f32".into(),
                "crc32:60:u32".into(),
            ],
            created_ms: now_ms(),
            last_write_ms: 0,
            rolled_hours: Vec::new(),
        }
    }

    fn read(dir: &Path) -> Self {
        fs::read_to_string(dir.join("manifest.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Manifest>(&s).ok())
            .unwrap_or_else(Manifest::new)
    }

    /// 原子替换（写临时文件 + rename），避免崩溃留下半个 manifest。
    fn write(&self, dir: &Path) {
        let tmp = dir.join("manifest.json.tmp");
        if let Ok(s) = serde_json::to_string_pretty(self) {
            if fs::write(&tmp, s).is_ok() {
                let _ = fs::rename(&tmp, dir.join("manifest.json"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 写线程
// ---------------------------------------------------------------------------

static STARTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// 写线程跑完收尾流程后置位；供 [`shutdown`] 同步等待。
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);

/// 启动后台写线程（进程内只启动一次）。
pub fn start(_app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(run);
}

/// 应用退出前调用：请求写线程 flush + sync 收尾，**并等它做完**（最多 2 秒）。
///
/// 为什么要等：窗口关闭后进程会立刻退出，而写线程每 200 ms 才检查一次退出标志；
/// 只置标志不等，进程常常在写线程醒来之前就没了 —— 实测会丢掉最后一个 15 秒批次，
/// 且 `manifest.last_write_ms` 停在打开时刻。等待后收尾是确定的。
/// 最多等 2 秒：收尾只做一次写 + 一次 fsync（实测 0.3 ms 量级），正常在毫秒级完成。
pub fn shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
    if !STARTED.load(Ordering::SeqCst) {
        return; // 写线程没起来（启动即降级退出），无需等待
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while !SHUTDOWN_DONE.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run() {
    run_writer();
    // 无论写线程从哪条路径退出（收尾完成 / 降级后放弃 / 窗口关闭），都放行 shutdown()
    SHUTDOWN_DONE.store(true, Ordering::SeqCst);
}

fn sleep_interruptible(d: Duration) {
    let step = Duration::from_millis(200);
    let mut left = d;
    while left > Duration::ZERO {
        if SHUTDOWN.load(Ordering::SeqCst) {
            return;
        }
        let s = left.min(step);
        std::thread::sleep(s);
        left = left.saturating_sub(s);
    }
}

fn run_writer() {
    loop {
        match Writer::open() {
            Ok(mut w) => {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    w.finish();
                    return;
                }
                w.serve();
                if SHUTDOWN.load(Ordering::SeqCst) {
                    return;
                }
            }
            Err(reason) => {
                // 降级为纯内存：内存环继续工作，这里退避重试，绝不阻塞采集
                set_status(move |s| {
                    s.writable = false;
                    s.reason = reason;
                });
                sleep_interruptible(RETRY_EVERY);
                if SHUTDOWN.load(Ordering::SeqCst) {
                    return;
                }
            }
        }
    }
}

struct Writer {
    dir: PathBuf,
    /// 独占锁句柄；活着就代表本实例是唯一写者。
    _lock: File,
    manifest: Manifest,
    cur: Option<File>,
    cur_hour: i64,
    last_ts_ms: i64,
    last_sync: Instant,
    last_scan: Instant,
    last_flush: Instant,
    failures: u32,
    bytes_total: u64,
    seg_raw: u32,
    seg_roll: u32,
    oldest_ms: i64,
}

impl Writer {
    fn open() -> Result<Writer, String> {
        Writer::open_dir(&resolve_dir())
    }

    /// 在指定目录上打开写者（测试用同一路径，保证验证的是生产代码）。
    fn open_dir(dir: &Path) -> Result<Writer, String> {
        let dir = dir.to_path_buf();
        fs::create_dir_all(&dir).map_err(|e| format!("无法创建历史目录 {}：{}", dir.display(), e))?;

        // 单实例：share_mode(0) 独占，第二个实例会拿到 sharing violation
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .share_mode(0)
            .open(dir.join("history.lock"))
            .map_err(|e| format!("另一个 Win-Top 实例正在写入历史（history.lock 被占用）：{}", e))?;

        let manifest = Manifest::read(&dir);
        let now = now_ms();
        // 段键必须是**本地对齐**，与 seg_name/parse_seg 的本地渲染/解析一致（见 local_hour_start）
        let cur_hour = local_hour_start(now);

        let cur_path = dir.join(seg_name(SegKind::Raw, cur_hour));
        let last_ts_ms = last_ts_in(&cur_path, RAW_BYTES).unwrap_or(now);

        let mut w = Writer {
            dir,
            _lock: lock,
            manifest,
            cur: None,
            cur_hour,
            last_ts_ms,
            last_sync: Instant::now(),
            last_scan: Instant::now(),
            last_flush: Instant::now(),
            failures: 0,
            bytes_total: 0,
            seg_raw: 0,
            seg_roll: 0,
            oldest_ms: 0,
        };
        w.open_segment()?;
        w.scan();
        w.roll_pending();
        w.enforce();
        w.manifest.last_write_ms = w.last_ts_ms;
        w.manifest.write(&w.dir);
        w.publish_ok();
        Ok(w)
    }

    fn open_segment(&mut self) -> Result<(), String> {
        let path = self.dir.join(seg_name(SegKind::Raw, self.cur_hour));
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("无法打开段文件 {}：{}", path.display(), e))?;
        self.cur = Some(f);
        Ok(())
    }

    /// 扫描目录：重建段索引、统计字节与最旧时间。
    fn scan(&mut self) {
        let mut segs = list_segments(&self.dir);
        // 当前段仍在写入中，Windows 的目录项长度可能是陈旧的（写入后才更新），
        // 用打开句柄自己的元数据修正，避免 status.bytes 在 flush 之后瞬间回落到 0。
        if let (Some(f), Some(cur)) = (
            self.cur.as_ref(),
            segs.iter_mut()
                .find(|s| s.kind == SegKind::Raw && s.start_ms == self.cur_hour),
        ) {
            if let Ok(md) = f.metadata() {
                cur.bytes = md.len();
            }
        }
        self.bytes_total = segs.iter().map(|s| s.bytes).sum();
        self.seg_raw = segs.iter().filter(|s| s.kind == SegKind::Raw).count() as u32;
        self.seg_roll = segs.iter().filter(|s| s.kind == SegKind::Roll1m).count() as u32;
        self.oldest_ms = segs
            .first()
            .and_then(|s| first_ts_in(&s.path, s.kind.record_bytes()))
            .unwrap_or(0);
    }

    /// 保留期 + 硬上限淘汰（段粒度、最旧优先）。
    fn enforce(&mut self) {
        let segs = list_segments(&self.dir);
        for path in plan_eviction(&segs, now_ms(), CAP_BYTES) {
            let _ = fs::remove_file(&path);
        }
        self.scan();
    }

    /// 把尚未聚合的、已完成小时的 RAW 段聚合成 1 分钟段（每次最多 6 段，避免长时间占用）。
    fn roll_pending(&mut self) {
        let cur_hour = local_hour_start(now_ms());
        let mut todo: Vec<SegmentInfo> = list_segments(&self.dir)
            .into_iter()
            .filter(|s| s.kind == SegKind::Raw && s.start_ms < cur_hour)
            .filter(|s| !self.manifest.rolled_hours.contains(&s.start_ms))
            .collect();
        todo.sort_by_key(|s| s.start_ms);
        todo.truncate(6);
        for seg in todo {
            if let Ok(bytes) = fs::read(&seg.path) {
                let rolled = rollup_bytes(&bytes);
                if !rolled.is_empty() {
                    // 日段键同样本地对齐（否则 seg_name→parse_seg 往返会差一个时区偏移）
                    let day = local_day_start(seg.start_ms);
                    let path = self.dir.join(seg_name(SegKind::Roll1m, day));
                    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                        if f.write_all(&rolled).is_ok() {
                            let _ = f.sync_all();
                        }
                    }
                }
            }
            self.manifest.rolled_hours.push(seg.start_ms);
        }
        // 只保留最近 400 条（更早的已超出聚合层保留期）
        let n = self.manifest.rolled_hours.len();
        if n > 400 {
            self.manifest.rolled_hours.drain(0..n - 400);
        }
    }

    fn publish_ok(&self) {
        set_status(|s| {
            s.enabled = true;
            s.writable = true;
            s.reason.clear();
            s.dir = self.dir.to_string_lossy().to_string();
            s.bytes = self.bytes_total;
            s.oldest_ms = self.oldest_ms;
            s.newest_ms = self.last_ts_ms;
            s.cap_bytes = CAP_BYTES;
            s.raw_segments = self.seg_raw;
            s.roll_segments = self.seg_roll;
        });
    }

    fn degrade(&mut self, reason: String) {
        self.cur = None;
        set_status(move |s| {
            s.writable = false;
            s.reason = reason;
        });
    }

    /// 写线程主循环；返回即表示需要重建（外层退避重试）。
    fn serve(&mut self) {
        loop {
            std::thread::sleep(TICK);
            if SHUTDOWN.load(Ordering::SeqCst) {
                let _ = self.flush_now();
                self.finish();
                return;
            }
            if self.last_flush.elapsed() >= FLUSH_EVERY {
                self.last_flush = Instant::now();
                match self.flush_now() {
                    Ok(()) => self.failures = 0,
                    Err(e) => {
                        self.failures += 1;
                        if self.failures >= MAX_CONSECUTIVE_FAILURES {
                            self.degrade(e);
                            return;
                        }
                    }
                }
            }
            if self.last_sync.elapsed() >= SYNC_EVERY {
                self.last_sync = Instant::now();
                self.sync_now();
                self.manifest.last_write_ms = self.last_ts_ms;
                self.manifest.write(&self.dir);
                self.publish_ok();
            }
            if self.last_scan.elapsed() >= SCAN_EVERY {
                self.last_scan = Instant::now();
                self.roll_pending();
                self.enforce();
                self.manifest.write(&self.dir);
                self.publish_ok();
            }
        }
    }

    fn finish(&mut self) {
        self.sync_now();
        self.manifest.last_write_ms = self.last_ts_ms;
        self.manifest.write(&self.dir);
        self.publish_ok();
    }

    fn sync_now(&mut self) {
        if let Some(f) = self.cur.as_ref() {
            let _ = f.sync_all();
        }
    }

    fn flush_now(&mut self) -> Result<(), String> {
        // 段轮转（跨本地整点；必须与 open() 用同一套本地对齐，否则会每 tick 误判轮转）
        let hour = local_hour_start(now_ms());
        if hour != self.cur_hour {
            self.sync_now();
            let _ = self.cur.take();
            self.cur_hour = hour;
            self.open_segment()?;
            self.roll_pending();
            self.enforce();
            self.manifest.write(&self.dir);
        }

        // 磁盘余量闸门（磁盘将满时这个程序必须第一个退让）
        if let Some(free) = free_bytes(&self.dir) {
            if free < MIN_FREE_BYTES {
                return Err(format!(
                    "磁盘可用空间不足（{:.2} GiB < 1 GiB），已暂停历史持久化",
                    free as f64 / 1073741824.0
                ));
            }
        }

        let batch = crate::history::snapshot_since(self.last_ts_ms);
        if batch.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(batch.len() * RAW_BYTES);
        let mut last = self.last_ts_ms;
        for s in &batch {
            if s.ts_ms <= last {
                continue;
            }
            let mut rec = [0u8; RAW_BYTES];
            encode_raw(&raw_from_snapshot(s), &mut rec);
            buf.extend_from_slice(&rec);
            last = s.ts_ms;
        }
        if buf.is_empty() {
            return Ok(());
        }
        let written = buf.len() as u64;
        let Some(f) = self.cur.as_mut() else {
            return Err("段文件句柄已失效".to_string());
        };
        f.write_all(&buf)
            .map_err(|e| format!("写入段文件失败：{}", e))?;
        self.bytes_total = self.bytes_total.saturating_add(written);
        self.last_ts_ms = last;
        if self.oldest_ms == 0 {
            self.oldest_ms = last;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 单元测试（纯逻辑 + 合成数据，不依赖真实 %LOCALAPPDATA%、不等 7 天）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wintop-hist-test-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    fn snap(ts_ms: i64, cpu: f64) -> MetricsSnapshot {
        MetricsSnapshot {
            ts: hms(ts_ms),
            ts_ms,
            cpu,
            mem_load: 42,
            mem_used_gb: 12.5,
            mem_total_gb: 31.25,
            mem_page_total_gb: 36.0,
            mem_page_used_gb: 16.5,
            disk_read_bps: 1024.5,
            disk_write_bps: 2048.25,
            net_total_bps: 4096.75,
            gpu: crate::gpu_proc::GpuHistSample {
                available: true,
                gpu_pct: 33.5,
                luid: "0x00000000:0x0000D969".to_string(),
                adapters: Vec::new(),
            },
        }
    }

    /// 测试辅助：把一个 RAW 段（64 B/条）写到 `hour_ms` 命名的段文件里。
    fn write_raw_segment(dir: &Path, hour_ms: i64, recs: &[(i64, f64)]) {
        let mut buf = Vec::with_capacity(recs.len() * RAW_BYTES);
        for (ts, cpu) in recs {
            let mut rec = [0u8; RAW_BYTES];
            encode_raw(&raw_from_snapshot(&snap(*ts, *cpu)), &mut rec);
            buf.extend_from_slice(&rec);
        }
        fs::write(dir.join(seg_name(SegKind::Raw, hour_ms)), &buf).unwrap();
    }

    /// 测试辅助：把一个 ROLL 段（80 B/条）写到 `day_ms` 命名的段文件里。
    fn write_roll_segment(dir: &Path, day_ms: i64, recs: &[(i64, f32)]) {
        let mut buf = Vec::with_capacity(recs.len() * ROLL_BYTES);
        for (ts, pct) in recs {
            let rec = RollRec {
                ts_ms: *ts,
                v: [*pct, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                cpu_max: *pct,
                samples: 60,
                ..Default::default()
            };
            let mut b = [0u8; ROLL_BYTES];
            encode_roll(&rec, &mut b);
            buf.extend_from_slice(&b);
        }
        fs::write(dir.join(seg_name(SegKind::Roll1m, day_ms)), &buf).unwrap();
    }

    #[test]
    fn raw_record_roundtrip_is_exact() {
        let s = snap(1_700_000_000_000, 12.5);
        let rec = raw_from_snapshot(&s);
        let mut buf = [0u8; RAW_BYTES];
        encode_raw(&rec, &mut buf);
        let back = decode_raw(&buf).expect("应能解码");
        assert_eq!(back.ts_ms, s.ts_ms);
        assert_eq!(back.mem_load, s.mem_load);
        assert_eq!(back.gpu.available, true);
        assert_eq!(back.gpu.pct, 33.5f32);
        assert_eq!(back.gpu.luid, 0x0000_0000_0000_D969);
        // f32 精度：有理半值可精确表示
        assert_eq!(back.v[0], 12.5f32);
        assert_eq!(back.v[1], 12.5f32);
        assert_eq!(back.v[2], 31.25f32);
        assert_eq!(back.v[5], 1024.5f32);
        // 往返到 MetricsSnapshot 也一致（除了未持久化的 adapters）
        let s2 = raw_to_snapshot(&back);
        assert_eq!(s2.cpu, 12.5);
        assert_eq!(s2.gpu.gpu_pct, 33.5);
        assert_eq!(s2.gpu.luid, "0x00000000:0x0000D969");
        assert!(s2.gpu.adapters.is_empty());
        assert_eq!(s2.ts_ms, s.ts_ms);
    }

    #[test]
    fn corrupt_record_fails_crc() {
        let rec = raw_from_snapshot(&snap(1_700_000_000_000, 50.0));
        let mut buf = [0u8; RAW_BYTES];
        encode_raw(&rec, &mut buf);
        assert!(decode_raw(&buf).is_some());
        // 翻一位载荷 → CRC 必须报错
        buf[8] ^= 0xFF;
        assert!(decode_raw(&buf).is_none(), "载荷被改动后 CRC 应失配");
        // 翻转 CRC 自身
        let mut buf2 = [0u8; RAW_BYTES];
        encode_raw(&rec, &mut buf2);
        buf2[60] ^= 0x01;
        assert!(decode_raw(&buf2).is_none());
        // 截断也必须是 None（不 panic）
        assert!(decode_raw(&buf[..40]).is_none());
    }

    #[test]
    fn roll_record_roundtrip_is_exact() {
        let r = RollRec {
            ts_ms: 1_700_000_040_000,
            v: [10.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
            gpu_luid: 0x0000_0000_0000_D969,
            cpu_max: 88.5,
            gpu_pct_avg: 12.25,
            gpu_pct_max: 40.5,
            samples: 60,
            mem_load_avg: 55,
            gpu_available: true,
        };
        let mut buf = [0u8; ROLL_BYTES];
        encode_roll(&r, &mut buf);
        assert_eq!(buf.len(), ROLL_BYTES);
        let back = decode_roll(&buf).expect("应能解码");
        assert_eq!(back.ts_ms, r.ts_ms);
        assert_eq!(back.cpu_max, 88.5);
        assert_eq!(back.gpu_pct_avg, 12.25);
        assert_eq!(back.gpu_pct_max, 40.5);
        assert_eq!(back.samples, 60);
        assert_eq!(back.mem_load_avg, 55);
        assert_eq!(back.gpu_luid, r.gpu_luid);
        assert!(back.gpu_available);
        assert_eq!(back.v, r.v);
        let mut bad = buf;
        bad[48] ^= 0xFF;
        assert!(decode_roll(&bad).is_none());
    }

    #[test]
    fn segment_names_roundtrip_and_unknowns_ignored() {
        let hour = local_hour_start(1_700_000_000_000);
        let name = seg_name(SegKind::Raw, hour);
        assert!(name.starts_with("v2-raw-") && name.ends_with(".raw"));
        let (kind, ms) = parse_seg(&name).expect("应能解析");
        assert_eq!(kind, SegKind::Raw);
        assert_eq!(ms, hour);

        let day = local_day_start(1_700_000_000_000);
        let rname = seg_name(SegKind::Roll1m, day);
        assert!(rname.starts_with("v2-roll1m-") && rname.ends_with(".roll"));
        assert_eq!(parse_seg(&rname), Some((SegKind::Roll1m, day)));

        // 未知/损坏文件名一律忽略（绝不删）
        assert!(parse_seg("history.lock").is_none());
        assert!(parse_seg("manifest.json").is_none());
        assert!(parse_seg("v1-raw-20261018T19.raw").is_none());
        assert!(parse_seg("v2-raw-garbage.raw").is_none());
    }

    #[test]
    fn cross_segment_read_concatenates_in_order() {
        let dir = tmp_dir("xseg");
        let hour = local_hour_start(1_700_000_000_000);
        // 两个相邻小时段，各 3 条
        for (i, h) in [hour, hour + RAW_SPAN_MS].iter().enumerate() {
            let mut buf = Vec::new();
            for k in 0..3 {
                let ts = h + (k as i64) * 1000 + (i as i64) * 10;
                let mut rec = [0u8; RAW_BYTES];
                encode_raw(&raw_from_snapshot(&snap(ts, 10.0 + k as f64)), &mut rec);
                buf.extend_from_slice(&rec);
            }
            fs::write(dir.join(seg_name(SegKind::Raw, *h)), &buf).unwrap();
        }
        let segs = list_segments(&dir);
        assert_eq!(segs.len(), 2);
        assert!(segs[0].start_ms < segs[1].start_ms);
        let total: usize = segs.iter().map(|s| s.bytes as usize / RAW_BYTES).sum();
        assert_eq!(total, 6);
        // 跨段拼接读取（直接走 list_segments 的顺序）
        let mut ts_all = Vec::new();
        for s in &segs {
            let b = fs::read(&s.path).unwrap();
            for c in b.chunks_exact(RAW_BYTES) {
                ts_all.push(decode_raw(c).unwrap().ts_ms);
            }
        }
        let mut sorted = ts_all.clone();
        sorted.sort();
        assert_eq!(ts_all, sorted, "跨段拼接必须天然按时间升序");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_and_cap_evict_oldest_first() {
        let now = 1_800_000_000_000i64;
        let mk = |kind: SegKind, start: i64, bytes: u64| SegmentInfo {
            kind,
            start_ms: start,
            path: PathBuf::from(seg_name(kind, start)),
            bytes,
        };
        // 8 天前的 raw（超 7 天保留期）+ 今天 3 个 raw + 1 个 roll
        let segs = vec![
            mk(SegKind::Raw, now - 8 * 86_400_000, 1000),
            mk(SegKind::Raw, now - 3 * 3_600_000, 1000),
            mk(SegKind::Raw, now - 2 * 3_600_000, 1000),
            mk(SegKind::Raw, now - 3_600_000, 1000),
            mk(SegKind::Roll1m, now - 86_400_000, 1000),
        ];
        // 只按保留期：删 1 个过期的 raw
        let doomed = plan_eviction(&segs, now, u64::MAX);
        assert_eq!(doomed.len(), 1);
        assert!(doomed[0].to_string_lossy().contains("v2-raw-"));

        // 上限压到只能留 2 个段（=2000 B）→ 先删 raw 且最旧优先
        let doomed = plan_eviction(&segs, now, 2000);
        let names: Vec<String> = doomed.iter().map(|p| p.to_string_lossy().to_string()).collect();
        assert_eq!(doomed.len(), 3, "保留期内应删 3 个：1 个过期 + 2 个超上限: {names:?}");
        assert!(names[0].contains("v2-raw-"));
        // 剩下的必须是最新的 2 个之一
        assert!(!names.iter().any(|n| n.contains("v2-roll1m")));
    }

    #[test]
    fn rollup_aggregates_per_minute_bucket() {
        let base = 1_700_000_000_000i64.div_euclid(ROLL_BUCKET_MS) * ROLL_BUCKET_MS;
        let mut raw = Vec::new();
        // 桶 1：3 条（cpu 10/90/20），桶 2：2 条（cpu 5/7）
        for (i, cpu) in [10.0f64, 90.0, 20.0].iter().enumerate() {
            let mut rec = [0u8; RAW_BYTES];
            encode_raw(&raw_from_snapshot(&snap(base + i as i64 * 1000, *cpu)), &mut rec);
            raw.extend_from_slice(&rec);
        }
        // 中间插一条坏记录，必须被跳过而不是中断聚合
        raw.extend_from_slice(&[0xAB; RAW_BYTES]);
        for (i, cpu) in [5.0f64, 7.0].iter().enumerate() {
            let ts = base + ROLL_BUCKET_MS + i as i64 * 1000;
            let mut rec = [0u8; RAW_BYTES];
            encode_raw(&raw_from_snapshot(&snap(ts, *cpu)), &mut rec);
            raw.extend_from_slice(&rec);
        }
        let out = rollup_bytes(&raw);
        assert_eq!(out.len(), 2 * ROLL_BYTES, "应产出 2 个 1 分钟桶");
        let b1 = decode_roll(&out[..ROLL_BYTES]).unwrap();
        assert_eq!(b1.ts_ms, base);
        assert_eq!(b1.samples, 3);
        assert_eq!(b1.cpu_max, 90.0);
        assert!((b1.v[0] - 40.0).abs() < 1e-3, "均值 (10+90+20)/3 = 40，实际 {}", b1.v[0]);
        let b2 = decode_roll(&out[ROLL_BYTES..]).unwrap();
        assert_eq!(b2.ts_ms, base + ROLL_BUCKET_MS);
        assert_eq!(b2.samples, 2);
        assert_eq!(b2.cpu_max, 7.0);
        assert!((b2.v[0] - 6.0).abs() < 1e-3);
    }

    #[test]
    fn downsample_respects_limit_order_and_keeps_spikes() {
        // 1000 个点，其中一个是尖峰
        let mut pts = Vec::new();
        for i in 0..1000i64 {
            let cpu = if i == 512 { 99.0 } else { 1.0 };
            pts.push(snap(1_700_000_000_000 + i * 1000, cpu));
        }
        let span = 1000 * 1000;
        let mut ds = Downsampler::new(span, 100);
        for p in pts {
            ds.push(p);
        }
        let out = ds.finish();
        assert!(out.len() <= 100, "抽稀后 {} 应 ≤ max_points", out.len());
        let mut sorted = out.clone();
        sorted.sort_by_key(|s| s.ts_ms);
        let ts: Vec<i64> = out.iter().map(|s| s.ts_ms).collect();
        assert_eq!(ts, sorted.iter().map(|s| s.ts_ms).collect::<Vec<_>>(), "必须按 ts_ms 升序");
        assert!(
            out.iter().any(|s| s.cpu == 99.0),
            "尖峰必须被保留（min/max 包络）"
        );
    }

    #[test]
    fn downsample_empty_and_tiny_inputs() {
        let ds = Downsampler::new(60_000, 10);
        assert!(ds.finish().is_empty(), "空输入应返回空");
        let mut ds = Downsampler::new(60_000, 1);
        ds.push(snap(1_700_000_000_000, 5.0));
        ds.push(snap(1_700_000_001_000, 6.0));
        let out = ds.finish();
        assert_eq!(out.len(), 1, "max_points=1 时只回 1 个点");
        assert_eq!(out[0].cpu, 6.0, "max_points=1 时返回全局最大值（尖峰优先）");
    }

    /// 30 天窗口的实测行为：`read_span` 对 >7 天跨度读 1 分钟聚合层 → 30 天 = 43,200 点。
    /// 断言桶数/包络点数/无截断/全局极值保留（极值刻意放在**最后一个桶**，
    /// 正是旧实现会被 truncate 裁掉的位置）。
    #[test]
    fn thirty_day_window_keeps_global_extremes_without_truncation() {
        let n = 43_200i64; // 30 天 × 每分钟一条
        let start = 1_700_000_000_000i64;
        let span = (n - 1) * 60_000;
        let mut ds = Downsampler::new(span, 800);
        let (bucket_ms, buckets) = (ds.bucket_ms, ds.buckets);
        for i in 0..n {
            let t = start + i * 60_000;
            let cpu = if i == n - 1 {
                97.25 // 全局 max，最后一个点（旧实现最容易被裁掉）
            } else if i == n - 2 {
                -5.5 // 全局 min，倒数第二个点
            } else {
                20.0 + 10.0 * (i as f64 * 0.001).sin()
            };
            ds.push(snap(t, cpu));
        }
        let out = ds.finish();
        println!(
            "30 天场景：输入 {n} 点 → 输出 {} 点（max_points=800，桶数上限 {buckets}，桶宽 {bucket_ms} ms，2×桶数={}）",
            out.len(),
            2 * buckets
        );
        assert!(out.len() <= 800, "输出 {} 点超过 max_points=800", out.len());
        assert!(
            out.len() <= 2 * buckets as usize,
            "包络点数必须 ≤ 2×桶数（旧实现会到 2×(桶数+1)）"
        );
        let ts: Vec<i64> = out.iter().map(|p| p.ts_ms).collect();
        let mut sorted = ts.clone();
        sorted.sort();
        assert_eq!(ts, sorted, "必须按 ts_ms 升序");
        assert!(
            out.iter().any(|p| p.cpu == 97.25),
            "30 天窗口的全局 max 必须在结果里"
        );
        assert!(
            out.iter().any(|p| p.cpu == -5.5),
            "30 天窗口的全局 min 必须在结果里"
        );
    }

    /// 回归（确定性复现，不依赖任何时钟）：包络点数曾可能超过 max_points，
    /// 末尾的 `truncate` 会把全局极值裁掉。旧实现按 epoch 绝对对齐分桶，下面的 3 个点
    /// 恰好跨在 10_000 ms 边界两侧 → 产出 3 个包络点 → 截断到 2 后全局 max 丢失。
    #[test]
    fn downsample_bucket_straddle_keeps_both_global_extremes() {
        let boundary = 1_700_000_010_000i64; // 10_000 的整数倍 → 旧实现的桶边界
        let ts = [boundary - 1000, boundary + 1000, boundary + 2000];
        let mut ds = Downsampler::new(10_000, 2);
        for (i, t) in ts.iter().enumerate() {
            ds.push(snap(*t, 10.0 + i as f64)); // 10.0 / 11.0 / 12.0
        }
        let out = ds.finish();
        let got: Vec<f64> = out.iter().map(|p| p.cpu).collect();
        assert!(out.len() <= 2, "不得超过 max_points：实际 {}", out.len());
        assert!(out.iter().any(|p| p.cpu == 10.0), "全局 min 必须保留：{got:?}");
        assert!(out.iter().any(|p| p.cpu == 12.0), "全局 max 必须保留：{got:?}");
    }

    /// 30 天窗口走**生产读盘路径**（跨度 >7 天 → 自动选 1 分钟聚合层）：
    /// 验证分层选择、段文件拼接、抽稀上限、以及"全局极值必在结果里"。
    /// 合成数据写在临时目录，不依赖真实 %LOCALAPPDATA%。
    #[test]
    fn read_span_thirty_days_uses_roll_tier_and_keeps_extremes() {
        let dir = tmp_dir("span30d");
        let now = TEST_NOW;
        let days = 30i64;
        let per_day = 1_440i64; // 1 分钟一条
        let start_day = local_day_start(now - (days - 1) * ROLL_SPAN_MS);

        let mut recs: Vec<(i64, f32)> = Vec::new();
        for d in 0..days {
            let day = local_day_start(start_day + d * ROLL_SPAN_MS);
            for m in 0..per_day {
                let ts = day + m * 60_000;
                if ts > now {
                    continue;
                }
                recs.push((ts, 20.0 + 5.0 * ((ts / 60_000) as f64 * 0.01).sin() as f32));
            }
        }
        recs.sort_by_key(|r| r.0);
        assert!(recs.len() > 40_000, "应覆盖约 43,200 分钟记录：{}", recs.len());
        // 全局极值刻意放在**最后两个点**（旧实现 truncate 会从尾部裁掉）
        let last = recs.len() - 1;
        recs[last].1 = 97.25;
        recs[last - 1].1 = -5.5;

        // 按"天"写成真实的 v2-roll1m 段文件
        let mut day_files = std::collections::BTreeMap::new();
        for (ts, pct) in &recs {
            let day = local_day_start(*ts);
            let rec = RollRec {
                ts_ms: *ts,
                v: [*pct, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                cpu_max: *pct,
                samples: 60,
                ..Default::default()
            };
            let mut buf = [0u8; ROLL_BYTES];
            encode_roll(&rec, &mut buf);
            day_files.entry(day).or_insert_with(Vec::new).extend_from_slice(&buf);
        }
        for (day, bytes) in &day_files {
            fs::write(dir.join(seg_name(SegKind::Roll1m, *day)), bytes).unwrap();
        }
        assert!(day_files.len() >= 28, "应写出约 30 个日段：{}", day_files.len());

        let out = read_span_at(&dir, (days * 86_400) as u64, 800, now);
        println!(
            "30 天 read_span：输入 {} 条 1 分钟记录（{} 个日段）→ 输出 {} 点（上限 800）",
            recs.len(),
            day_files.len(),
            out.len()
        );
        assert!(!out.is_empty());
        assert!(out.len() <= 800, "输出 {} 点超过 max_points", out.len());
        let ts: Vec<i64> = out.iter().map(|p| p.ts_ms).collect();
        let mut sorted = ts.clone();
        sorted.sort();
        assert_eq!(ts, sorted, "必须按 ts_ms 升序");
        assert!(
            out.iter().any(|p| (p.cpu - 97.25).abs() < 1e-6),
            "30 天窗口全局 max 必须在结果里"
        );
        assert!(
            out.iter().any(|p| (p.cpu + 5.5).abs() < 1e-6),
            "30 天窗口全局 min 必须在结果里"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 全新安装场景：只有原始层（无任何聚合层），7 天与 30 天窗口都必须能读回点。
    /// 30 天跨度 > 7 天 → 走"聚合层为空 → 回退原始层"分支。
    #[test]
    fn long_window_falls_back_to_raw_when_rollup_is_empty() {
        let dir = tmp_dir("fallback");
        let now = TEST_NOW;
        // 只有 40 分钟原始数据（每分钟一条）
        let recs: Vec<(i64, f64)> = (0..40i64)
            .map(|i| (now - 40 * 60_000 + i * 60_000, 10.0 + i as f64))
            .collect();
        let hour = local_hour_start(recs[0].0);
        write_raw_segment(&dir, hour, &recs);

        // ① 恰好 7 天（span == 原始层保留期 → 直接走原始层）
        let w7 = read_span_at(&dir, 7 * 86_400, 800, now);
        assert!(!w7.is_empty(), "只有原始层时 7 天窗口不能为空");

        // ② 30 天（>7 天 → 无聚合层 → 必须回退原始层）
        let w30 = read_span_at(&dir, 30 * 86_400, 800, now);
        println!(
            "只有原始层（40 分钟数据，{} 条）：7 天窗口 {} 点，30 天窗口 {} 点",
            recs.len(),
            w7.len(),
            w30.len()
        );
        assert!(!w30.is_empty(), "聚合层为空时 30 天窗口必须回退原始层，不能返回空数组");
        assert!(w30.len() <= 800);
        let ts: Vec<i64> = w30.iter().map(|p| p.ts_ms).collect();
        let mut sorted = ts.clone();
        sorted.sort();
        assert_eq!(ts, sorted, "必须按 ts_ms 升序");
        assert!(w30.iter().any(|p| p.cpu == 10.0), "全局 min 必须保留");
        assert!(w30.iter().any(|p| p.cpu == 49.0), "全局 max 必须保留");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 两层同时存在：聚合层覆盖最近 3 天 + 原始层补它之前的 2 小时空隙。
    /// 断言：不重复计数（ts 唯一）、两层都被读到、极值保留、升序、≤ max_points。
    #[test]
    fn mixed_tiers_do_not_double_count() {
        let dir = tmp_dir("mixed");
        let now = TEST_NOW;
        let roll_span_days = 3i64;
        let roll_start = now - roll_span_days * 86_400_000;

        // 聚合层：最近 3 天，每分钟一条；全局 **min** 放在聚合层里
        let mut roll_recs: Vec<(i64, f32)> = Vec::new();
        let mut t = roll_start;
        while t <= now {
            roll_recs.push((t, 20.0));
            t += 60_000;
        }
        roll_recs[10].1 = -3.5;
        let mut per_day: std::collections::BTreeMap<i64, Vec<(i64, f32)>> = Default::default();
        for r in &roll_recs {
            per_day.entry(local_day_start(r.0)).or_default().push(*r);
        }
        for (day, recs) in &per_day {
            write_roll_segment(&dir, *day, recs);
        }

        // 原始层：只覆盖聚合层之前的 2 小时空隙（每分钟一条）；全局 **max** 放在这里
        let raw_end = roll_start - 60_000;
        let mut raw_recs: Vec<(i64, f64)> = Vec::new();
        for i in 0..120i64 {
            raw_recs.push((raw_end - (119 - i) * 60_000, 20.0));
        }
        raw_recs[5].1 = 91.5;
        let hour = local_hour_start(raw_recs[0].0);
        write_raw_segment(&dir, hour, &raw_recs);

        let out = read_span_at(&dir, 30 * 86_400, 800, now);
        println!(
            "混合两层（roll {} 条 + raw {} 条）→ 30 天窗口 {} 点",
            roll_recs.len(),
            raw_recs.len(),
            out.len()
        );
        assert!(out.len() <= 800);
        // 不重复计数：同一个 ts_ms 不得出现两次
        let mut ts: Vec<i64> = out.iter().map(|p| p.ts_ms).collect();
        let before = ts.len();
        ts.sort_unstable();
        ts.dedup();
        assert_eq!(ts.len(), before, "输出中出现了重复的 ts_ms（两层重叠了）");
        // 升序
        let ordered: Vec<i64> = out.iter().map(|p| p.ts_ms).collect();
        let mut sorted = ordered.clone();
        sorted.sort();
        assert_eq!(ordered, sorted, "必须按 ts_ms 升序");
        // 两层都真的被读到了：raw 段贡献全局 max(91.5)，roll 段贡献全局 min(−3.5)
        assert!(
            out.iter().any(|p| p.cpu == 91.5),
            "原始层（回退段）的极值必须在结果里"
        );
        assert!(
            out.iter().any(|p| (p.cpu + 3.5).abs() < 1e-6),
            "聚合层的极值必须在结果里"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 聚合层**落后于 now**（当前这一小时还没 rollup）时，右端必须用原始层补上。
    ///
    /// 主断言是"最新数据被补进来"（输出右端覆盖到 `now` 附近）；顺带断言极值保留 ——
    /// 那是 [`read_span`] 文档里明确承诺的不变量（`max_points >= 2` 时全局 min/max 必在结果里），
    /// 不是这条测试额外的强要求。
    #[test]
    fn raw_tail_is_appended_when_rollup_lags_behind_now() {
        let dir = tmp_dir("tail");
        let now = TEST_NOW;
        // 聚合层覆盖 [now-3d, now-2h]
        let roll_start = now - 3 * 86_400_000;
        let roll_end = now - 2 * 3_600_000;
        let mut roll_recs: Vec<(i64, f32)> = Vec::new();
        let mut t = roll_start;
        while t <= roll_end {
            roll_recs.push((t, 20.0));
            t += 60_000;
        }
        roll_recs[10].1 = -3.5; // 全局 min 在聚合层
        let mut per_day: std::collections::BTreeMap<i64, Vec<(i64, f32)>> = Default::default();
        for r in &roll_recs {
            per_day.entry(local_day_start(r.0)).or_default().push(*r);
        }
        for (day, recs) in &per_day {
            write_roll_segment(&dir, *day, recs);
        }
        // 原始层只覆盖 (now-2h, now] 这段"聚合层尚未产出"的尾部
        let mut raw_recs: Vec<(i64, f64)> = Vec::new();
        for i in 0..120i64 {
            raw_recs.push((roll_end + (i + 1) * 60_000, 20.0));
        }
        let last_i = raw_recs.len() - 1;
        raw_recs[last_i].1 = 91.5; // 全局 max 在原始尾部（最新的一条）
        let hour = local_hour_start(raw_recs[0].0);
        write_raw_segment(&dir, hour, &raw_recs);

        let out = read_span_at(&dir, 30 * 86_400, 800, now);
        println!(
            "聚合层落后 now：roll {} 条 + raw 尾部 {} 条 → 30 天窗口 {} 点",
            roll_recs.len(),
            raw_recs.len(),
            out.len()
        );
        assert!(out.len() <= 800);
        let mut ts: Vec<i64> = out.iter().map(|p| p.ts_ms).collect();
        let before = ts.len();
        ts.sort_unstable();
        ts.dedup();
        assert_eq!(ts.len(), before, "输出中出现了重复的 ts_ms");
        assert!(
            out.iter().any(|p| p.cpu == 91.5),
            "原始尾部贡献的全局 max 必须在结果里（否则右端缺最新数据）"
        );
        assert!(
            out.iter().any(|p| (p.cpu + 3.5).abs() < 1e-6),
            "聚合层的全局 min 必须保留"
        );
        // 右端确实补到了"现在"附近
        let newest = out.iter().map(|p| p.ts_ms).max().unwrap_or(0);
        assert!(newest >= now - 1000, "输出右端应覆盖到 now 附近，实际最新 {newest} vs now {now}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 固定的"当前时刻"（本地 JST 2023-11-15 **07:13:20**）。
    ///
    /// 特意选在**本地 09:00 之前**：旧实现用 UTC 整日当段键、却按本地时间渲染/解析段名，
    /// 往返偏移 −9 小时，使得读路径的"段与窗口相交"判断在本地 00:00–09:00 这个时段
    /// 把最早一个聚合段整段跳过（极值随之丢失）——也就是 Lead 观察到的"到点必红"。
    /// 现在：
    ///   1. 段键一律本地对齐（`local_hour_start` / `local_day_start`），与时段无关；
    ///   2. 读路径的 `now` 是显式参数，测试钉死为这个常量 → **完全不读真实时钟**。
    /// 若有人退回 UTC 对齐的段键，这些测试会在任何时刻稳定失败（而不是等到某个时段）。
    const TEST_NOW: i64 = 1_700_000_000_000;

    /// 段名往返必须精确：段键用本地对齐时，`seg_name → parse_seg` 必须还原出同一个时刻。
    /// 这条直接锁死上面那个 −9 小时偏移的根因。
    #[test]
    fn segment_name_roundtrip_is_exact_for_local_aligned_keys() {
        // 覆盖本地一天的多个时刻（含旧实现出错的 00:00–09:00 时段）
        for offset_h in [0i64, 1, 7, 9, 12, 23] {
            let ms = TEST_NOW + offset_h * 3_600_000;
            for kind in [SegKind::Raw, SegKind::Roll1m] {
                let key = match kind {
                    SegKind::Raw => local_hour_start(ms),
                    SegKind::Roll1m => local_day_start(ms),
                };
                let name = seg_name(kind, key);
                let (pk, pms) = parse_seg(&name).expect("段名必须可解析");
                assert_eq!(pk, kind);
                assert_eq!(
                    pms, key,
                    "段名 {name} 往返不精确：解析回 {pms} 但段键是 {key}（时区对齐问题）"
                );
            }
        }
        // 顺手确认 UTC 整日键确实**不是**本地对齐值（旧实现就是拿它当段键用的）
        let utc_day = TEST_NOW.div_euclid(ROLL_SPAN_MS) * ROLL_SPAN_MS;
        assert_ne!(
            utc_day,
            local_day_start(TEST_NOW),
            "本机时区偏移为 0 时这条断言无意义，可忽略"
        );
    }

    /// 读路径的确定性：同一份数据 + 同一个固定 `now`，反复读必须逐点相同。
    #[test]
    fn read_span_is_deterministic_for_a_fixed_now() {
        let dir = tmp_dir("determinism");
        let now = TEST_NOW;
        // 原始层：最近 2 小时，每 30 秒一条，含一个尖峰
        let mut recs: Vec<(i64, f64)> = (0..240i64)
            .map(|i| (now - 7_200_000 + i * 30_000, 5.0 + (i % 7) as f64))
            .collect();
        recs[100].1 = 88.0;
        let hour = local_hour_start(recs[0].0);
        write_raw_segment(&dir, hour, &recs);

        let a = read_span_at(&dir, 7_200, 40, now);
        let b = read_span_at(&dir, 7_200, 40, now);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.ts_ms, y.ts_ms, "同一固定 now 两次读取的点位必须一致");
            assert_eq!(x.cpu, y.cpu);
        }
        assert!(!a.is_empty());
        assert!(a.len() <= 40);
        assert!(a.iter().any(|p| p.cpu == 88.0), "尖峰必须保留");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unwritable_directory_degrades_to_memory_only() {
        // 让 <tmp>/afile 是一个文件，则 <tmp>/afile/history 无法创建
        let base = tmp_dir("degrade");
        let blocker = base.join("afile");
        fs::write(&blocker, b"x").unwrap();
        let target = blocker.join("history");
        assert!(fs::create_dir_all(&target).is_err(), "该路径必须无法创建");
        // 目录不存在时读盘必须安全返回空，而不是 panic / 阻塞
        assert!(list_segments(&target).is_empty());
        assert!(read_span_at(&target, 3600, 100, TEST_NOW).is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tmp_dir("lock");
        let p = dir.join("history.lock");
        let a = OpenOptions::new().create(true).write(true).share_mode(0).open(&p);
        assert!(a.is_ok(), "第一次独占应成功");
        let b = OpenOptions::new().create(true).write(true).share_mode(0).open(&p);
        assert!(b.is_err(), "第二个实例必须拿不到锁（sharing violation）");
        drop(a);
        let c = OpenOptions::new().create(true).write(true).share_mode(0).open(&p);
        assert!(c.is_ok(), "前一个句柄释放后应可再次获取");
        drop(c);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_roundtrip_and_layout_fields() {
        let dir = tmp_dir("manifest");
        let m = Manifest::new();
        assert_eq!(m.layout, "v2");
        assert_eq!(m.record_bytes as usize, RAW_BYTES);
        assert_eq!(m.roll_record_bytes as usize, ROLL_BYTES);
        assert_eq!(m.raw_fields.len(), 14);
        m.write(&dir);
        let got = Manifest::read(&dir);
        assert_eq!(got.layout, "v2");
        assert_eq!(got.raw_fields, m.raw_fields);
        // 坏 manifest 必须能回落而不是 panic
        fs::write(dir.join("manifest.json"), b"{ not json").unwrap();
        assert_eq!(Manifest::read(&dir).layout, "v2");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_survives_random_garbage() {
        // 随机（但确定）字节流不得 panic
        let mut buf = vec![0u8; RAW_BYTES];
        for i in 0..buf.len() {
            buf[i] = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        assert!(decode_raw(&buf).is_none());
        let mut rbuf = vec![0u8; ROLL_BYTES];
        for i in 0..rbuf.len() {
            rbuf[i] = (i as u8).wrapping_mul(91).wrapping_add(7);
        }
        assert!(decode_roll(&rbuf).is_none());
    }

    /// 端到端（临时目录，覆盖生产代码路径）：内存环 → 写者 flush → 段文件 → status → 读回。
    ///
    /// 说明：写者的段轮转用的是真实时钟（`Writer` 内部 `now_ms()`，无注入点），所以这里的数据
    /// 用"刚刚"的时间戳；但**断言只取决于数据相对 now 的位置**，与具体时刻无关。
    /// 读路径的 `now` 显式传入（= 数据末尾），不再由读函数内部读时钟。
    #[test]
    fn writer_pipeline_flush_status_and_readback() {
        let dir = tmp_dir("pipeline");
        let base = now_ms() - 5_000;
        let read_now = base + 5_000;
        // 往内存环注入 3 条（生产路径 collector → history::push）
        for i in 0..3 {
            crate::history::push(snap(base + i * 1000, 10.0 + i as f64));
        }

        let mut w = Writer::open_dir(&dir).expect("应能打开写者");
        w.last_ts_ms = base - 1; // 假装环里的 3 条都是新的
        let ring_n = crate::history::snapshot_since(base - 1).len();
        w.flush_now().expect("flush 应成功");
        w.scan();
        w.publish_ok();
        assert_eq!(ring_n, 3, "内存环里必须有刚推入的 3 条");
        assert_eq!(w.dir, dir, "写者必须落在指定目录");

        let segs = list_segments(&dir);
        assert_eq!(segs.len(), 1, "应恰好产生一个 RAW 段");
        assert_eq!(segs[0].kind, SegKind::Raw);
        assert_eq!(segs[0].bytes, 3 * RAW_BYTES as u64, "3 条 × 64 B");

        let st = status();
        assert!(st.enabled);
        assert!(st.writable, "写成功后 status 必须为可写");
        assert!(st.reason.is_empty());
        assert_eq!(st.bytes, 3 * RAW_BYTES as u64);
        assert_eq!(st.oldest_ms, base);
        assert_eq!(st.newest_ms, base + 2000);
        assert_eq!(st.raw_segments, 1);

        // 走生产读路径读回（跨度 10 秒 → 每条各自成桶，min/max 包络不会合并它们）
        let pts = read_span_at(&dir, 10, 100, read_now);
        assert_eq!(pts.len(), 3);
        assert_eq!(
            pts.iter().map(|p| p.ts_ms).collect::<Vec<_>>(),
            vec![base, base + 1000, base + 2000]
        );
        assert_eq!(pts[2].cpu, 12.0);
        assert!(pts[0].gpu.available, "GPU 标量必须被持久化");
        assert_eq!(pts[0].gpu.gpu_pct, 33.5);

        // 抽稀：把 max_points 压到 2，min/max 包络应保留 10.0 与 12.0 两个端点
        let few = read_span_at(&dir, 10, 2, read_now);
        assert!(few.len() <= 2);
        assert!(few.iter().any(|p| p.cpu == 10.0) && few.iter().any(|p| p.cpu == 12.0));

        // manifest 必须落盘且记录布局
        let m = Manifest::read(&dir);
        assert_eq!(m.layout, "v2");
        assert_eq!(m.record_bytes as usize, RAW_BYTES);

        // 再次 flush 不应重复写入（靠 last_ts_ms 去重）
        w.flush_now().expect("第二次 flush 应成功");
        assert_eq!(
            list_segments(&dir).iter().map(|s| s.bytes).sum::<u64>(),
            3 * RAW_BYTES as u64,
            "重复 flush 不得写重复记录"
        );

        drop(w);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 保留期淘汰必须真的删文件（合成一个过期的 RAW 段）。
    /// 段键一律用本地对齐（与写入侧一致）；保留判断是"9 天 > 7 天"，与具体时刻无关。
    #[test]
    fn enforce_deletes_expired_segment_files() {
        let dir = tmp_dir("enforce");
        let old_hour = local_hour_start(now_ms() - 9 * 86_400_000);
        let mut buf = Vec::new();
        let mut rec = [0u8; RAW_BYTES];
        encode_raw(&raw_from_snapshot(&snap(old_hour + 1000, 1.0)), &mut rec);
        buf.extend_from_slice(&rec);
        let old_path = dir.join(seg_name(SegKind::Raw, old_hour));
        fs::write(&old_path, &buf).unwrap();
        // 再来一个"当前"段，必须被保留
        let cur_hour = local_hour_start(now_ms());
        let cur_path = dir.join(seg_name(SegKind::Raw, cur_hour));
        fs::write(&cur_path, &buf).unwrap();
        assert_eq!(list_segments(&dir).len(), 2);

        let mut w = Writer::open_dir(&dir).expect("应能打开写者");
        w.enforce();
        assert!(!old_path.exists(), "超过 7 天的 RAW 段必须被删除");
        assert!(cur_path.exists(), "保留期内的段不得被删除");
        // 注意：open_dir 里会先 roll_pending() 再 enforce() —— 过期的 RAW 会先被聚合成
        // 1 分钟段（保住粗粒度数据），然后才删原始段。这是有意行为，故只数 RAW 段。
        let raws = list_segments(&dir)
            .into_iter()
            .filter(|s| s.kind == SegKind::Raw)
            .count();
        assert_eq!(raws, 1, "RAW 段只应剩当前这个");
        let roll_day = local_day_start(old_hour);
        assert!(
            dir.join(seg_name(SegKind::Roll1m, roll_day)).exists(),
            "过期 RAW 应已被聚合成 1 分钟段"
        );
        drop(w);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 真实落盘验证（默认 `#[ignore]`）：先运行 release 应用 ≥16 秒，再执行
    /// `cargo test -- --ignored real_disk_roundtrip`。
    #[test]
    #[ignore]
    fn real_disk_roundtrip() {
        let dir = resolve_dir();
        println!("dir = {}", dir.display());
        let segs = list_segments(&dir);
        println!("segments = {}", segs.len());
        for s in &segs {
            assert_eq!(
                s.bytes % s.kind.record_bytes() as u64,
                0,
                "段大小必须是记录长度的整数倍：{:?}",
                s.path
            );
            println!("  {:?} {} B ({} 条)", s.kind, s.bytes, s.bytes / s.kind.record_bytes() as u64);
        }
        assert!(!segs.is_empty(), "先运行应用让它落盘，再跑这个测试");
        let m = Manifest::read(&dir);
        println!("manifest: layout={} raw={}B roll={}B rolled_hours={}", m.layout, m.record_bytes, m.roll_record_bytes, m.rolled_hours.len());
        // 走生产读路径（与 get_history_span 命令完全同一条代码）
        let pts = read_span(3600, 800);
        println!("read_span(3600, 800) -> {} 点", pts.len());
        assert!(!pts.is_empty(), "读盘必须能取到数据");
        assert!(pts.len() <= 800);
        let ts: Vec<i64> = pts.iter().map(|p| p.ts_ms).collect();
        let mut sorted = ts.clone();
        sorted.sort();
        assert_eq!(ts, sorted, "必须按 ts_ms 升序");
        println!(
            "首点 ts_ms={} cpu={:.1} gpu_avail={} | 末点 ts_ms={} cpu={:.1}",
            pts[0].ts_ms, pts[0].cpu, pts[0].gpu.available,
            pts[pts.len() - 1].ts_ms, pts[pts.len() - 1].cpu
        );
    }
}
