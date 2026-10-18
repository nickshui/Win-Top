//! gpuproc —— 原生 PDH per-process GPU 利用率 PoC（Win-Top task-1 调研用）
//!
//! 验证目标：
//!   1. 一个 PDH query + `PdhAddEnglishCounterW("\GPU Engine(*)\Utilization Percentage")`，
//!      用 `PdhGetFormattedCounterArrayW` 一次读回全部实例（不做 per-instance counter）。
//!   2. 解析实例名 `pid_10668_luid_0x00000000_0x0000d969_phys_0_eng_0_engtype_3d`
//!      → (pid, luid_hi, luid_lo, phys, eng, engtype)。
//!   3. 按 PID 聚合（sum / 单引擎 max / engtype 分解）与按 LUID（显卡）聚合。
//!   4. PID → 进程名走 NtQuerySystemInformation（不 spawn 子进程）。
//!   5. LUID → 显卡名走 DXGI `CreateDXGIFactory1` + `EnumAdapters1` + `GetDesc1`。
//!   6. 成本分解：`PdhCollectQueryData` / `PdhGetFormattedCounterArrayW` / 自身解析
//!      分开计时；对比 (a) 通配持久 query 与 (b) `PdhExpandWildCardPathW` + 只添加活跃实例；
//!      另用 3 / 47 / 688 实例的对照 query 观察成本是否随实例数增长。
//!   7. 退化情况：0 实例、PDH_CSTATUS_INVALID_DATA / PDH_INVALID_DATA、实例抖动。
//!
//! 运行（在 poc-native 目录）：`cargo run --release --bin gpuproc -- <输出文件>`
//! 全程只读系统计数器，不修改任何系统状态。
//!
//! 注意：windows-rs 0.58 未导出 `PDH_FMT_NOCAP100`，本文件按 pdh.h 的值 0x8000 自行定义，
//! 并专门做了一次「带/不带 NOCAP100」的取值对比（见输出 NOCAP100 段）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ERROR_NOT_FOUND};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhExpandWildCardPathW,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW, PdhRemoveCounter, PDH_FMT,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

const PATH_ENGINE: &str = "\\GPU Engine(*)\\Utilization Percentage";
const PATH_PROC_MEM: &str = "\\GPU Process Memory(*)\\Dedicated Usage";
const PATH_ADAPTER_MEM: &str = "\\GPU Adapter Memory(*)\\Dedicated Usage";

/// pdh.h: `#define PDH_FMT_NOCAP100 ((DWORD)0x00008000)`；windows-rs 0.58 未导出。
const PDH_FMT_NOCAP100_RAW: u32 = 0x0000_8000;
/// 主对比采样 tick 数（≥10）与采样间隔。
const TICKS: usize = 10;
const TICK_SLEEP_MS: u64 = 1000;
/// 方案 (b) 选活跃实例的上限与阈值。
const ACTIVE_MIN_PCT: f64 = 0.05;
const ACTIVE_CAP: usize = 64;

// ---------------------------------------------------------------------------
// 输出收集：stdout 与落盘文件内容逐字节一致
// ---------------------------------------------------------------------------

struct Out {
    buf: String,
}

impl Out {
    fn new() -> Self {
        Out { buf: String::new() }
    }
    fn line(&mut self, s: &str) {
        println!("{s}");
        self.buf.push_str(s);
        self.buf.push('\n');
    }
}

macro_rules! outln {
    ($o:expr, $($t:tt)*) => {{ let s = format!($($t)*); $o.line(&s); }};
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn stat_line(v: &[f64]) -> String {
    if v.is_empty() {
        return "n/a (无样本)".to_string();
    }
    let min = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = v.iter().sum::<f64>() / v.len() as f64;
    format!("min {min:.3} / avg {avg:.3} / max {max:.3}")
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// PDH 返回码 → 符号名（逐字报告错误码，不做猜测）
fn pdh_rc_name(rc: u32) -> &'static str {
    match rc {
        0x0000_0000 => "ERROR_SUCCESS",
        0x8000_07D0 => "PDH_CSTATUS_NO_MACHINE",
        0x8000_07D1 => "PDH_CSTATUS_NO_INSTANCE",
        0x8000_07D2 => "PDH_MORE_DATA",
        0x8000_07D5 => "PDH_NO_DATA",
        0xC000_0BB8 => "PDH_CSTATUS_NO_OBJECT",
        0xC000_0BB9 => "PDH_CSTATUS_NO_COUNTER",
        0xC000_0BBA => "PDH_CSTATUS_INVALID_DATA",
        0xC000_0BBB => "PDH_MEMORY_ALLOCATION_FAILURE",
        0xC000_0BBC => "PDH_INVALID_HANDLE",
        0xC000_0BBD => "PDH_INVALID_ARGUMENT",
        0xC000_0BBE => "PDH_FUNCTION_NOT_FOUND",
        0xC000_0BC6 => "PDH_INVALID_DATA",
        0xC000_0BD1 => "PDH_INVALID_PATH",
        _ => "PDH_UNKNOWN_CODE",
    }
}

fn rcs(rc: u32) -> String {
    format!("0x{rc:08X} ({})", pdh_rc_name(rc))
}

fn proc_cpu_100ns() -> i64 {
    unsafe {
        let mut c = FILETIME::default();
        let mut e = FILETIME::default();
        let mut k = FILETIME::default();
        let mut u = FILETIME::default();
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_ok() {
            let f = |x: &FILETIME| ((x.dwHighDateTime as u64) << 32) | x.dwLowDateTime as u64;
            (f(&k) + f(&u)) as i64
        } else {
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// 实例名解析
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct GpuInstance {
    raw: String,
    pid: u32,
    luid_hi: u32,
    luid_lo: u32,
    phys: u32,
    eng: u32,
    engtype: String,
    engtype_raw: String,
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let t = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(t, 16).ok()
}

/// 归一化 engtype。实测该计数器远不止 4 种：`3D` / `Copy` / `Compute 0|1|3` /
/// `High Priority 3D` / `High Priority Compute` / `Video Decode 1` / `VideoDecode` /
/// `Video JPEG 0` / `Video Codec 0` / `VideoProcessing` / `True Audio 0|1` / `Timer 0` /
/// `Security 1` / `LegacyOverlay` / 以及空串。末尾数字是同类型的不同物理引擎序号，归一化时去掉。
fn canonical_engtype(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return "(empty)".to_string();
    }
    let base = match s.rsplit_once(' ') {
        Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => s,
    };
    match base.to_ascii_lowercase().as_str() {
        "3d" => "3D",
        "high priority 3d" => "HighPriority3D",
        "compute" => "Compute",
        "high priority compute" => "HighPriorityCompute",
        "copy" => "Copy",
        "video decode" | "videodecode" => "VideoDecode",
        "video encode" | "videoencode" => "VideoEncode",
        "video jpeg" => "VideoJPEG",
        "video codec" => "VideoCodec",
        "video processing" | "videoprocessing" => "VideoProcessing",
        "video scaling" | "videoscaling" => "VideoScaling",
        "true audio" => "TrueAudio",
        "timer" => "Timer",
        "security" => "Security",
        "legacyoverlay" => "LegacyOverlay",
        "other" => "Other",
        _ => return format!("?{raw}"),
    }
    .to_string()
}

/// `pid_10668_luid_0x00000000_0x0000d969_phys_0_eng_0_engtype_3d`
fn parse_instance(name: &str) -> Option<GpuInstance> {
    let toks: Vec<&str> = name.split('_').collect();
    let mut pid: Option<u32> = None;
    let mut hi: Option<u32> = None;
    let mut lo: Option<u32> = None;
    let mut phys: Option<u32> = None;
    let mut eng: Option<u32> = None;
    let mut engtype_raw: Option<String> = None;
    let mut i = 0usize;
    while i < toks.len() {
        match toks[i] {
            "pid" if i + 1 < toks.len() => {
                pid = toks[i + 1].parse::<u32>().ok();
                i += 2;
            }
            "luid" if i + 2 < toks.len() => {
                hi = parse_hex_u32(toks[i + 1]);
                lo = parse_hex_u32(toks[i + 2]);
                i += 3;
            }
            "phys" if i + 1 < toks.len() => {
                phys = toks[i + 1].parse::<u32>().ok();
                i += 2;
            }
            "eng" if i + 1 < toks.len() => {
                eng = toks[i + 1].parse::<u32>().ok();
                i += 2;
            }
            "engtype" if i + 1 < toks.len() => {
                engtype_raw = Some(toks[i + 1..].join("_"));
                i = toks.len();
            }
            _ => i += 1,
        }
    }
    let (pid, hi, lo) = (pid?, hi?, lo?);
    let engtype_raw = engtype_raw.unwrap_or_else(|| "?".to_string());
    Some(GpuInstance {
        raw: name.to_string(),
        pid,
        luid_hi: hi,
        luid_lo: lo,
        phys: phys.unwrap_or(0),
        eng: eng.unwrap_or(0),
        engtype: canonical_engtype(&engtype_raw),
        engtype_raw,
    })
}

// ---------------------------------------------------------------------------
// PDH 封装
// ---------------------------------------------------------------------------

struct ArrayRead {
    rc: u32,
    items: Vec<(String, f64)>,
    bad_status: BTreeMap<u32, usize>,
    buf_bytes: u32,
    item_count: u32,
}

/// 一次 `PdhGetFormattedCounterArrayW`：先问大小，再读全量数组。
/// `PDH_FMT_COUNTERVALUE_ITEM_W` 需要 8 字节对齐，故用 Vec<u64> 做缓冲。
fn read_array(hcounter: isize, fmt: u32) -> ArrayRead {
    let mut res = ArrayRead {
        rc: 0,
        items: Vec::new(),
        bad_status: BTreeMap::new(),
        buf_bytes: 0,
        item_count: 0,
    };
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let mut rc = unsafe {
        PdhGetFormattedCounterArrayW(hcounter, PDH_FMT(fmt), &mut size, &mut count, None)
    };
    // 0 实例时 PDH 可能直接返回 PDH_CSTATUS_NO_INSTANCE / PDH_NO_DATA
    if rc != 0 && rc != PDH_MORE_DATA {
        res.rc = rc;
        return res;
    }
    if count == 0 || size == 0 {
        res.rc = rc;
        return res;
    }
    res.buf_bytes = size;
    let mut buf: Vec<u64> = vec![0u64; (size as usize).div_ceil(8)];
    let mut size2 = size;
    let mut count2 = count;
    rc = unsafe {
        PdhGetFormattedCounterArrayW(
            hcounter,
            PDH_FMT(fmt),
            &mut size2,
            &mut count2,
            Some(buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
        )
    };
    if rc != 0 {
        // 抖动导致 size 变化时会再拿到 PDH_MORE_DATA，交由调用方决定是否重试
        res.rc = rc;
        return res;
    }
    let slice = unsafe {
        std::slice::from_raw_parts(
            buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
            count2 as usize,
        )
    };
    for it in slice {
        let name = unsafe { String::from_utf16_lossy(it.szName.as_wide()) };
        let cs = it.FmtValue.CStatus;
        // PDH_CSTATUS_VALID_DATA(0) 与 PDH_CSTATUS_NEW_DATA(1) 都算有效
        if cs == 0 || cs == 1 {
            let v = unsafe { it.FmtValue.Anonymous.doubleValue };
            res.items.push((name, v));
        } else {
            *res.bad_status.entry(cs).or_insert(0) += 1;
        }
    }
    res.item_count = count2;
    res.rc = 0;
    res
}

struct PdhQuery {
    label: String,
    path: String,
    hquery: isize,
    counters: Vec<isize>,
}

impl PdhQuery {
    fn open(label: &str, path: &str, out: &mut Out) -> Option<PdhQuery> {
        let mut hq: isize = 0;
        let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut hq) };
        if rc != 0 {
            outln!(out, "[FAIL] PdhOpenQueryW({path}) → {}", rcs(rc));
            return None;
        }
        let mut hc: isize = 0;
        let w = wide(path);
        let rc = unsafe { PdhAddEnglishCounterW(hq, PCWSTR::from_raw(w.as_ptr()), 0, &mut hc) };
        if rc != 0 {
            outln!(
                out,
                "[FAIL] PdhAddEnglishCounterW({path}) → {}",
                rcs(rc)
            );
            unsafe { PdhCloseQuery(hq) };
            return None;
        }
        outln!(out, "[ OK ] {label}: PdhOpenQueryW + PdhAddEnglishCounterW → rc=0");
        Some(PdhQuery {
            label: label.to_string(),
            path: path.to_string(),
            hquery: hq,
            counters: vec![hc],
        })
    }

    fn collect(&self) -> (u32, Duration) {
        let t = Instant::now();
        let rc = unsafe { PdhCollectQueryData(self.hquery) };
        (rc, t.elapsed())
    }

    fn close(&self, out: &mut Out) {
        let rc = unsafe { PdhCloseQuery(self.hquery) };
        outln!(
            out,
            "  [close] {} → {}",
            self.label,
            rcs(rc)
        );
    }
}

#[derive(Default, Clone)]
struct TickRow {
    idx: usize,
    sleep_ms: u128,
    wall: Duration,
    collect: Duration,
    read: Duration,
    parse: Duration,
    rc_collect: u32,
    rc_read: u32,
    bad_counters: usize,
    items: usize,
    bad_items: usize,
    buf_bytes: u32,
    cpu_us: i64,
    n_pids: usize,
    n_luids: usize,
    sum_util: f64,
    max_util: f64,
    values: Vec<(GpuInstance, f64)>,
}

/// 跑 n 个 tick：每个 tick = PdhCollectQueryData(计时) → 逐 counter 读数组(计时) → 解析(计时)。
/// 调用前必须已做过一次 baseline collect，否则 rate 计数器无基准对。
fn run_ticks(
    q: &PdhQuery,
    ticks: usize,
    sleep_ms: u64,
    fmt: u32,
    parse_names: bool,
) -> Vec<TickRow> {
    let mut rows = Vec::with_capacity(ticks);
    for idx in 0..ticks {
        let cpu0 = proc_cpu_100ns();
        let mut slept = 0u128;
        if idx > 0 && sleep_ms > 0 {
            let t = Instant::now();
            std::thread::sleep(Duration::from_millis(sleep_ms));
            slept = t.elapsed().as_millis();
        }
        let t_tick = Instant::now();
        let (rc_collect, collect) = q.collect();
        let mut read = Duration::ZERO;
        let mut raw: Vec<(String, f64)> = Vec::new();
        let mut bad_items = 0usize;
        let mut rc_read = 0u32;
        let mut bad_counters = 0usize;
        let mut buf_bytes = 0u32;
        for hc in &q.counters {
            let t = Instant::now();
            let ar = read_array(*hc, fmt);
            read += t.elapsed();
            if ar.rc != 0 {
                bad_counters += 1;
                if rc_read == 0 {
                    rc_read = ar.rc;
                }
            }
            bad_items += ar.bad_status.values().sum::<usize>();
            buf_bytes = buf_bytes.max(ar.buf_bytes);
            raw.extend(ar.items);
        }
        let t_parse = Instant::now();
        let mut values: Vec<(GpuInstance, f64)> = Vec::new();
        if parse_names {
            for (n, v) in &raw {
                if let Some(inst) = parse_instance(n) {
                    values.push((inst, *v));
                }
            }
        }
        let parse = t_parse.elapsed();
        let n_pids = values
            .iter()
            .map(|(i, _)| i.pid)
            .collect::<BTreeSet<_>>()
            .len();
        let n_luids = values
            .iter()
            .map(|(i, _)| (i.luid_hi, i.luid_lo))
            .collect::<BTreeSet<_>>()
            .len();
        let sum_util: f64 = values.iter().map(|(_, v)| *v).sum::<f64>() + 0.0; // +0.0 归一化 -0.0
        let max_util = values.iter().map(|(_, v)| *v).fold(0.0f64, f64::max);
        let cpu_us = {
            let c = proc_cpu_100ns();
            if c < 0 || cpu0 < 0 {
                -1
            } else {
                (c - cpu0) / 10
            }
        };
        rows.push(TickRow {
            idx,
            sleep_ms: slept,
            wall: t_tick.elapsed(),
            collect,
            read,
            parse,
            rc_collect,
            rc_read,
            bad_counters,
            items: raw.len(),
            bad_items,
            buf_bytes,
            cpu_us,
            n_pids,
            n_luids,
            sum_util,
            max_util,
            values,
        });
    }
    rows
}

fn print_ticks(out: &mut Out, title: &str, rows: &[TickRow]) {
    outln!(out, "{title}");
    outln!(
        out,
        "  {:>3} {:>8} {:>10} {:>10} {:>9} {:>7} {:>6} {:>6} {:>6} {:>9} {:>9} {:>8}",
        "idx",
        "sleep_ms",
        "collect_ms",
        "array_ms",
        "parse_ms",
        "items",
        "baditm",
        "badcnt",
        "pids",
        "sum%",
        "max%",
        "cpu_ms"
    );
    for r in rows {
        outln!(
            out,
            "  {:>3} {:>8} {:>10.3} {:>10.3} {:>9.3} {:>7} {:>6} {:>6} {:>6} {:>9.2} {:>9.2} {:>8.3}",
            r.idx,
            r.sleep_ms,
            ms(r.collect),
            ms(r.read),
            ms(r.parse),
            r.items,
            r.bad_items,
            r.bad_counters,
            r.n_pids,
            r.sum_util,
            r.max_util,
            if r.cpu_us < 0 { f64::NAN } else { r.cpu_us as f64 / 1000.0 }
        );
        if r.rc_collect != 0 || r.rc_read != 0 {
            outln!(
                out,
                "      ↳ rc_collect={} rc_read={}",
                rcs(r.rc_collect),
                rcs(r.rc_read)
            );
        }
    }
}

struct PhaseStats {
    collect_ms: Vec<f64>,
    read_ms: Vec<f64>,
    parse_ms: Vec<f64>,
    total_ms: Vec<f64>,
    items: Vec<f64>,
    cpu_ms: Vec<f64>,
}

fn phase_stats(rows: &[TickRow]) -> PhaseStats {
    let mut s = PhaseStats {
        collect_ms: Vec::new(),
        read_ms: Vec::new(),
        parse_ms: Vec::new(),
        total_ms: Vec::new(),
        items: Vec::new(),
        cpu_ms: Vec::new(),
    };
    for r in rows {
        s.collect_ms.push(ms(r.collect));
        s.read_ms.push(ms(r.read));
        s.parse_ms.push(ms(r.parse));
        s.total_ms.push(ms(r.collect) + ms(r.read) + ms(r.parse));
        s.items.push(r.items as f64);
        if r.cpu_us >= 0 {
            s.cpu_ms.push(r.cpu_us as f64 / 1000.0);
        }
    }
    s
}

fn print_phase_summary(out: &mut Out, name: &str, rows: &[TickRow]) {
    let s = phase_stats(rows);
    outln!(out, "  {name}  ({n} ticks)", n = rows.len());
    outln!(out, "    collect : {} ms", stat_line(&s.collect_ms));
    outln!(out, "    array   : {} ms", stat_line(&s.read_ms));
    outln!(out, "    parse   : {} ms", stat_line(&s.parse_ms));
    outln!(out, "    total   : {} ms", stat_line(&s.total_ms));
    outln!(out, "    items   : {}", stat_line(&s.items));
    if !s.cpu_ms.is_empty() {
        outln!(
            out,
            "    自身 CPU: {} ms/tick (GetProcessTimes 差值)",
            stat_line(&s.cpu_ms)
        );
    }
    let wall: f64 = rows.iter().map(|r| ms(r.wall)).sum();
    outln!(
        out,
        "    采样窗口: 纯计算 {:.1} ms + sleep {} ms → wall {:.1} ms",
        s.total_ms.iter().sum::<f64>(),
        rows.iter().map(|r| r.sleep_ms).sum::<u128>(),
        wall + rows.iter().map(|r| r.sleep_ms).sum::<u128>() as f64
    );
}

// ---------------------------------------------------------------------------
// PdhExpandWildCardPathW
// ---------------------------------------------------------------------------

fn expand_wildcard(path: &str) -> (u32, Vec<String>, Duration) {
    let wp = wide(path);
    let t0 = Instant::now();
    let mut len: u32 = 0;
    let mut rc = unsafe {
        PdhExpandWildCardPathW(
            PCWSTR::null(),
            PCWSTR::from_raw(wp.as_ptr()),
            PWSTR::null(),
            &mut len,
            0,
        )
    };
    if rc != PDH_MORE_DATA {
        return (rc, Vec::new(), t0.elapsed());
    }
    let mut buf: Vec<u16> = vec![0u16; len as usize + 2];
    rc = unsafe {
        PdhExpandWildCardPathW(
            PCWSTR::null(),
            PCWSTR::from_raw(wp.as_ptr()),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
            0,
        )
    };
    if rc != 0 {
        return (rc, Vec::new(), t0.elapsed());
    }
    let mut items = Vec::new();
    let mut start = 0usize;
    for i in 0..buf.len() {
        if buf[i] == 0 {
            if i == start {
                break;
            }
            items.push(String::from_utf16_lossy(&buf[start..i]));
            start = i + 1;
        }
    }
    (0, items, t0.elapsed())
}

// ---------------------------------------------------------------------------
// 进程名（NtQuerySystemInformation，不 spawn 子进程）
// ---------------------------------------------------------------------------

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

fn enumerate_process_names(out: &mut Out) -> HashMap<u32, String> {
    let t0 = Instant::now();
    let mut len: u32 = 512 * 1024;
    let mut buf: Vec<u8> = vec![0u8; len as usize];
    loop {
        let mut ret: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buf.as_mut_ptr() as *mut c_void,
                len,
                &mut ret,
            )
        };
        if status.0 == STATUS_INFO_LENGTH_MISMATCH {
            len = ret.max(len * 2);
            buf = vec![0u8; len as usize];
            continue;
        }
        if status.is_ok() {
            break;
        }
        outln!(
            out,
            "[FAIL] NtQuerySystemInformation → 0x{:08X}（进程名不可用）",
            status.0
        );
        return HashMap::new();
    }
    let mut map: HashMap<u32, String> = HashMap::new();
    let mut offset = 0usize;
    loop {
        let info = unsafe { &*(buf.as_ptr().add(offset) as *const SYSTEM_PROCESS_INFORMATION) };
        let pid = info.UniqueProcessId.0 as u32;
        let name = if info.ImageName.Buffer.0.is_null() {
            if pid == 0 {
                "System Idle Process".to_string()
            } else {
                String::new()
            }
        } else {
            let chars = (info.ImageName.Length / 2) as usize;
            let slice = unsafe { std::slice::from_raw_parts(info.ImageName.Buffer.0, chars) };
            String::from_utf16_lossy(slice)
        };
        map.insert(pid, name);
        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
    outln!(
        out,
        "  NtQuerySystemInformation(SystemProcessInformation): {} 个进程, {:.3} ms",
        map.len(),
        ms(t0.elapsed())
    );
    map
}

// ---------------------------------------------------------------------------
// DXGI 显卡枚举
// ---------------------------------------------------------------------------

struct AdapterInfo {
    idx: u32,
    name: String,
    vendor: u32,
    device: u32,
    luid_hi: u32,
    luid_lo: u32,
    flags: u32,
    vram_mb: f64,
}

fn dxgi_probe(out: &mut Out) -> Vec<AdapterInfo> {
    let mut res = Vec::new();
    outln!(out, "== DXGI 显卡枚举（LUID → 显卡名）==");
    let factory = match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        Ok(f) => {
            outln!(out, "  CreateDXGIFactory1::<IDXGIFactory1>() 未做 COM 初始化 → OK");
            f
        }
        Err(e) => {
            outln!(
                out,
                "  CreateDXGIFactory1 未做 COM 初始化 → 失败 0x{:08X} {:?}",
                e.code().0 as u32,
                e
            );
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            outln!(
                out,
                "  CoInitializeEx(COINIT_MULTITHREADED) → 0x{:08X} (is_ok={})",
                hr.0 as u32,
                hr.is_ok()
            );
            match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
                Ok(f) => {
                    outln!(out, "  CoInitializeEx 后重试 CreateDXGIFactory1 → OK");
                    f
                }
                Err(e2) => {
                    outln!(
                        out,
                        "  CoInitializeEx 后重试仍失败 0x{:08X} {:?} → DXGI 不可用",
                        e2.code().0 as u32,
                        e2
                    );
                    return res;
                }
            }
        }
    };

    let mut idx = 0u32;
    loop {
        match unsafe { factory.EnumAdapters1(idx) } {
            Ok(ad) => {
                match unsafe { ad.GetDesc1() } {
                    Ok(d) => {
                        let end = d
                            .Description
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(d.Description.len());
                        let name = String::from_utf16_lossy(&d.Description[..end]);
                        outln!(
                            out,
                            "  [{}] {:?}  vendor=0x{:04X} device=0x{:04X} luid=0x{:08X}:0x{:08X} flags=0x{:X} vram={:.0} MB",
                            idx,
                            name,
                            d.VendorId,
                            d.DeviceId,
                            d.AdapterLuid.HighPart as u32,
                            d.AdapterLuid.LowPart,
                            d.Flags,
                            d.DedicatedVideoMemory as f64 / 1048576.0
                        );
                        res.push(AdapterInfo {
                            idx,
                            name,
                            vendor: d.VendorId,
                            device: d.DeviceId,
                            luid_hi: d.AdapterLuid.HighPart as u32,
                            luid_lo: d.AdapterLuid.LowPart,
                            flags: d.Flags,
                            vram_mb: d.DedicatedVideoMemory as f64 / 1048576.0,
                        });
                    }
                    Err(e) => outln!(
                        out,
                        "  [{idx}] GetDesc1 失败 0x{:08X} {:?}",
                        e.code().0 as u32,
                        e
                    ),
                }
                idx += 1;
                if idx > 64 {
                    outln!(out, "  [warn] 适配器数超过 64，停止枚举");
                    break;
                }
            }
            Err(e) => {
                if e.code() == DXGI_ERROR_NOT_FOUND {
                    break;
                }
                outln!(
                    out,
                    "  EnumAdapters1({idx}) 失败 0x{:08X} {:?} → 停止枚举",
                    e.code().0 as u32,
                    e
                );
                break;
            }
        }
    }
    outln!(out, "  DXGI 共 {} 个适配器", res.len());
    res
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let out_arg = std::env::args().nth(1);
    let mut out = Out::new();
    let t_all = Instant::now();

    // ---- 头 ----
    let st = unsafe { GetLocalTime() };
    outln!(
        out,
        "========================================================================"
    );
    outln!(out, " gpuproc — 原生 PDH per-process GPU 利用率 PoC（Win-Top task-1）");
    outln!(
        out,
        " 本地时间 : {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        st.wYear,
        st.wMonth,
        st.wDay,
        st.wHour,
        st.wMinute,
        st.wSecond,
        st.wMilliseconds
    );
    outln!(
        out,
        " build    : {} (debug_assertions={})",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        cfg!(debug_assertions)
    );
    outln!(out, " 计数器   : {PATH_ENGINE}");
    outln!(out, " 采样     : {TICKS} ticks × {TICK_SLEEP_MS} ms（主对比，含基线对）");
    outln!(out, " 输出文件 : {}", out_arg.clone().unwrap_or_else(|| "(stdout only)".into()));
    outln!(
        out,
        "========================================================================"
    );

    // ---- DXGI ----
    let adapters = dxgi_probe(&mut out);

    // ---- Phase A：通配持久 query ----
    outln!(out, "");
    outln!(
        out,
        "== Phase A: 通配持久 query（1 个 counter，PdhGetFormattedCounterArrayW 读全部实例）=="
    );
    let qa = match PdhQuery::open("A(wildcard engine)", PATH_ENGINE, &mut out) {
        Some(q) => q,
        None => {
            outln!(out, "!! Phase A 无法建立，终止。");
            finalize(out, out_arg, t_all);
            return;
        }
    };

    // 基线 collect + 第一次读（演示 rate 计数器缺基准对时的退化返回码）
    let (rc0, d0) = qa.collect();
    outln!(
        out,
        "  baseline PdhCollectQueryData → {} , {:.3} ms",
        rcs(rc0),
        ms(d0)
    );
    {
        let ar = read_array(qa.counters[0], PDH_FMT_DOUBLE.0);
        outln!(
            out,
            "  首次读取（只有 1 次 collect，缺基准对）: rc={} , item_count={} , items={} , bad_status={:?}",
            rcs(ar.rc),
            ar.item_count,
            ar.items.len(),
            ar.bad_status
                .iter()
                .map(|(k, v)| format!("{}×{}", rcs(*k), v))
                .collect::<Vec<_>>()
        );
    }

    let rows_a = run_ticks(&qa, TICKS, TICK_SLEEP_MS, PDH_FMT_DOUBLE.0, true);
    print_ticks(&mut out, "  Phase A 逐 tick（items=实例数, baditm=单项无效, badcnt=无效 counter 数）:", &rows_a);
    print_phase_summary(&mut out, "Phase A 统计", &rows_a);

    // 实例数是否抖动
    let items_a: Vec<usize> = rows_a.iter().map(|r| r.items).collect();
    let uniq_items: BTreeSet<usize> = items_a.iter().cloned().collect();
    outln!(
        out,
        "  实例数抖动: {} 种取值 {:?}（最小 {} / 最大 {}）",
        uniq_items.len(),
        items_a,
        items_a.iter().min().copied().unwrap_or(0),
        items_a.iter().max().copied().unwrap_or(0)
    );
    let pids_a: BTreeSet<u32> = rows_a
        .iter()
        .flat_map(|r| r.values.iter().map(|(i, _)| i.pid))
        .collect();
    let luids_a: BTreeSet<(u32, u32)> = rows_a
        .iter()
        .flat_map(|r| r.values.iter().map(|(i, _)| (i.luid_hi, i.luid_lo)))
        .collect();
    outln!(
        out,
        "  全窗口去重: {} 个 PID, {} 个 LUID",
        pids_a.len(),
        luids_a.len()
    );
    let max_single = rows_a
        .iter()
        .flat_map(|r| r.values.iter().map(|(_, v)| *v))
        .fold(0.0f64, f64::max);
    let max_pid_sum = rows_a
        .iter()
        .map(|r| {
            let mut m: BTreeMap<u32, f64> = BTreeMap::new();
            for (i, v) in &r.values {
                *m.entry(i.pid).or_insert(0.0) += *v;
            }
            m.values().cloned().fold(0.0f64, f64::max)
        })
        .fold(0.0f64, f64::max);
    outln!(
        out,
        "  观测到的单引擎最大值 {:.2}% ；单 PID 跨引擎求和最大值 {:.2}%（求和 >100% 属正常，非封顶值）",
        max_single,
        max_pid_sum
    );

    // ---- 实例名解析自检（pid / luid / phys / eng / engtype 全字段）----
    outln!(out, "");
    outln!(out, "== 实例名解析自检 ==");
    {
        let last_tick = rows_a.last().cloned().unwrap_or_default();
        outln!(
            out,
            "  最后一个 tick: 原始实例 {} 个，解析成功 {} 个（{:.2}%）",
            last_tick.items,
            last_tick.values.len(),
            if last_tick.items > 0 {
                last_tick.values.len() as f64 / last_tick.items as f64 * 100.0
            } else {
                0.0
            }
        );
        outln!(out, "  样例（原始实例名 → 解析结果）:");
        for (i, v) in last_tick.values.iter().take(5) {
            outln!(
                out,
                "    {}  →  pid={} luid=0x{:08X}:0x{:08X} phys={} eng={} engtype_raw={:?} engtype={} value={:.3}%",
                i.raw,
                i.pid,
                i.luid_hi,
                i.luid_lo,
                i.phys,
                i.eng,
                i.engtype_raw,
                i.engtype,
                v
            );
        }
        let mut mismatch = 0usize;
        for (i, _) in &last_tick.values {
            let expect = format!(
                "pid_{}_luid_0x{:08x}_0x{:08x}_phys_{}_eng_{}_engtype_{}",
                i.pid, i.luid_hi, i.luid_lo, i.phys, i.eng, i.engtype_raw
            );
            if !expect.eq_ignore_ascii_case(&i.raw) {
                mismatch += 1;
            }
        }
        outln!(
            out,
            "  往返重建（用解析出的字段重新拼实例名）不一致数: {}/{}",
            mismatch,
            last_tick.values.len()
        );
        let phys_set: BTreeSet<u32> = last_tick.values.iter().map(|(i, _)| i.phys).collect();
        let eng_max = last_tick.values.iter().map(|(i, _)| i.eng).max().unwrap_or(0);
        outln!(
            out,
            "  distinct phys={:?}；eng 索引范围 0..={}",
            phys_set,
            eng_max
        );
        outln!(
            out,
            "  每 tick distinct LUID: {:?}",
            rows_a.iter().map(|r| r.n_luids).collect::<Vec<_>>()
        );
        outln!(
            out,
            "  PdhGetFormattedCounterArrayW 缓冲峰值: {} bytes（最后 tick {} bytes）",
            rows_a.iter().map(|r| r.buf_bytes).max().unwrap_or(0),
            last_tick.buf_bytes
        );

        // engtype 命名表（生产实现需要完整映射）
        let mut ex: BTreeMap<String, (String, usize)> = BTreeMap::new();
        let mut raws: BTreeSet<String> = BTreeSet::new();
        for (i, _) in &last_tick.values {
            let e = ex.entry(i.engtype.clone()).or_insert((i.raw.clone(), 0));
            e.1 += 1;
            raws.insert(i.engtype_raw.clone());
        }
        outln!(out, "  engtype 规范名 → 样例实例名（最后 tick）:");
        for (k, (raw, n)) in &ex {
            outln!(out, "    {:<22} n={:<4} 例: {}", k, n, raw);
        }
        let unmapped: Vec<String> = raws
            .iter()
            .filter(|r| canonical_engtype(r).starts_with('?'))
            .cloned()
            .collect();
        outln!(
            out,
            "  engtype_raw 共 {} 种，未能归一化 {} 种 {:?}",
            raws.len(),
            unmapped.len(),
            unmapped
        );

        // (luid, eng) → engtype_raw 是否一一对应（解析自洽性检查）
        let mut eng_map: BTreeMap<(u32, u32, u32), BTreeSet<String>> = BTreeMap::new();
        for (i, _) in &last_tick.values {
            eng_map
                .entry((i.luid_hi, i.luid_lo, i.eng))
                .or_default()
                .insert(i.engtype_raw.clone());
        }
        let conflict = eng_map.values().filter(|s| s.len() > 1).count();
        outln!(
            out,
            "  (luid, eng) → engtype_raw 冲突数: {conflict}（0 = eng 索引与 engtype 一一对应）"
        );
    }

    // NOCAP100 对比：同一份已采集数据上分别读 512 与 512|0x8000
    outln!(out, "");
    outln!(out, "== PDH_FMT_NOCAP100 对比（windows-rs 0.58 未导出该常量，此处按 pdh.h 用 0x8000）==");
    {
        let mut max_diff = 0.0f64;
        let mut n_diff = 0usize;
        let mut n_cmp = 0usize;
        let mut rc_plain = 0u32;
        let mut rc_nocap = 0u32;
        for _ in 0..3 {
            let a = read_array(qa.counters[0], PDH_FMT_DOUBLE.0);
            let b = read_array(qa.counters[0], PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100_RAW);
            rc_plain = a.rc;
            rc_nocap = b.rc;
            let bm: HashMap<&str, f64> =
                b.items.iter().map(|(n, v)| (n.as_str(), *v)).collect();
            for (n, v) in &a.items {
                if let Some(v2) = bm.get(n.as_str()) {
                    n_cmp += 1;
                    let d = (v - v2).abs();
                    if d > 1e-9 {
                        n_diff += 1;
                    }
                    if d > max_diff {
                        max_diff = d;
                    }
                }
            }
        }
        outln!(
            out,
            "  三次对比读数: rc(plain)={} rc(NOCAP100)={} 比较项={} 差异项={} 最大绝对差={:.9}",
            rcs(rc_plain),
            rcs(rc_nocap),
            n_cmp,
            n_diff,
            max_diff
        );
        outln!(
            out,
            "  结论: {}",
            if n_diff == 0 {
                "两者完全一致 → 该计数器不存在 100% 封顶，主路径用 PDH_FMT_DOUBLE 即可；但仍不建议对求和结果做 100 封顶（跨引擎求和天然 >100%）"
            } else {
                "存在差异 → 必须显式带 NOCAP100，否则会被 PDH 封顶"
            }
        );
    }

    // ---- 进程名 ----
    outln!(out, "");
    outln!(out, "== PID → 进程名（NtQuerySystemInformation）==");
    let names = enumerate_process_names(&mut out);
    let unresolved: Vec<u32> = pids_a
        .iter()
        .cloned()
        .filter(|p| names.get(p).map(|s| s.is_empty()).unwrap_or(true))
        .collect();
    outln!(
        out,
        "  Phase A 的 {} 个 PID 中，无进程名（已退出/无权限）: {} 个 {:?}",
        pids_a.len(),
        unresolved.len(),
        unresolved
    );

    // ---- Phase F：展开 + 抖动 ----
    outln!(out, "");
    outln!(out, "== Phase F: PdhExpandWildCardPathW 成本与实例抖动 ==");
    let (rc_exp, all_paths, exp_dur) = expand_wildcard(PATH_ENGINE);
    outln!(
        out,
        "  展开 {PATH_ENGINE} → {} , {} 个实例路径, 首次耗时 {:.3} ms",
        rcs(rc_exp),
        all_paths.len(),
        ms(exp_dur)
    );
    let mut exp_times = vec![ms(exp_dur)];
    let mut prev: BTreeSet<String> = all_paths.iter().cloned().collect();
    let mut added_total = 0usize;
    let mut removed_total = 0usize;
    for i in 0..9 {
        let (rc, paths, d) = expand_wildcard(PATH_ENGINE);
        exp_times.push(ms(d));
        let cur: BTreeSet<String> = paths.iter().cloned().collect();
        let added = cur.difference(&prev).count();
        let removed = prev.difference(&cur).count();
        added_total += added;
        removed_total += removed;
        outln!(
            out,
            "  re-expand #{:<2} rc={} n={:<4} {:.3} ms  (+{} / -{})",
            i + 1,
            rcs(rc),
            cur.len(),
            ms(d),
            added,
            removed
        );
        prev = cur;
    }
    outln!(
        out,
        "  重展开耗时: {} ms ；9 次重展开累计新增 {} / 消失 {} 个实例",
        stat_line(&exp_times),
        added_total,
        removed_total
    );

    // 方案 (b) 选活跃实例
    let last_a = rows_a.last().cloned().unwrap_or_default();
    let mut cand: Vec<(GpuInstance, f64)> = last_a
        .values
        .iter()
        .filter(|(_, v)| *v >= ACTIVE_MIN_PCT)
        .cloned()
        .collect();
    cand.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let total_last: f64 = last_a.values.iter().map(|(_, v)| *v).sum();
    let selected_sum: f64 = cand.iter().map(|(_, v)| *v).sum();
    let n_before_cap = cand.len();
    cand.truncate(ACTIVE_CAP);
    let sel_sum_capped: f64 = cand.iter().map(|(_, v)| *v).sum();
    let sel_paths: Vec<String> = cand
        .iter()
        .map(|(i, _)| format!("\\GPU Engine({})\\Utilization Percentage", i.raw))
        .collect();
    outln!(
        out,
        "  活跃筛选: 阈值 ≥{ACTIVE_MIN_PCT}% → {} 个实例（覆盖该 tick 总和的 {:.1}%）；封顶 {ACTIVE_CAP} 后 {} 个（覆盖 {:.1}%）",
        n_before_cap,
        if total_last > 0.0 {
            selected_sum / total_last * 100.0
        } else {
            0.0
        },
        sel_paths.len(),
        if total_last > 0.0 {
            sel_sum_capped / total_last * 100.0
        } else {
            0.0
        }
    );

    // ---- Phase B(b)：展开后只添加活跃实例 ----
    outln!(out, "");
    outln!(
        out,
        "== Phase B(b): PdhExpandWildCardPathW + 仅添加 {} 个活跃实例 ==",
        sel_paths.len()
    );
    let mut rows_b: Vec<TickRow> = Vec::new();
    let mut add_avg_ms = 0.0f64;
    let mut add_fail = 0usize;
    if sel_paths.is_empty() {
        outln!(out, "  [skip] 活跃实例为 0（当前无 GPU 活动），方案 (b) 无法对比");
    } else {
        let mut hq: isize = 0;
        let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut hq) };
        if rc != 0 {
            outln!(out, "[FAIL] PdhOpenQueryW → {}", rcs(rc));
        } else {
            let mut counters: Vec<isize> = Vec::with_capacity(sel_paths.len());
            let t_add = Instant::now();
            for p in &sel_paths {
                let w = wide(p);
                let mut hc: isize = 0;
                let rc = unsafe {
                    PdhAddEnglishCounterW(hq, PCWSTR::from_raw(w.as_ptr()), 0, &mut hc)
                };
                if rc != 0 {
                    add_fail += 1;
                    if add_fail <= 3 {
                        outln!(out, "  [FAIL] PdhAddEnglishCounterW({p}) → {}", rcs(rc));
                    }
                } else {
                    counters.push(hc);
                }
            }
            let add_dur = t_add.elapsed();
            add_avg_ms = ms(add_dur) / sel_paths.len() as f64;
            outln!(
                out,
                "  PdhAddEnglishCounterW × {} → 成功 {} 失败 {} ，总 {:.3} ms ，均值 {:.4} ms/个",
                sel_paths.len(),
                counters.len(),
                add_fail,
                ms(add_dur),
                add_avg_ms
            );
            let qb = PdhQuery {
                label: "B(expanded active)".to_string(),
                path: format!("<{} 个具体实例>", counters.len()),
                hquery: hq,
                counters,
            };
            let (rc1, d1) = qb.collect();
            outln!(
                out,
                "  baseline PdhCollectQueryData → {} , {:.3} ms",
                rcs(rc1),
                ms(d1)
            );
            {
                let mut bad = 0usize;
                let mut tot = 0usize;
                let mut first_rc = 0u32;
                for hc in &qb.counters {
                    let ar = read_array(*hc, PDH_FMT_DOUBLE.0);
                    if ar.rc != 0 {
                        bad += 1;
                        if first_rc == 0 {
                            first_rc = ar.rc;
                        }
                    }
                    tot += ar.items.len();
                }
                outln!(
                    out,
                    "  首次读取: 有数据 counter={}/{} items={} 首个错误={}",
                    qb.counters.len() - bad,
                    qb.counters.len(),
                    tot,
                    rcs(first_rc)
                );
            }
            rows_b = run_ticks(&qb, TICKS, TICK_SLEEP_MS, PDH_FMT_DOUBLE.0, true);
            print_ticks(
                &mut out,
                "  Phase B(b) 逐 tick:",
                &rows_b,
            );
            print_phase_summary(&mut out, "Phase B(b) 统计", &rows_b);
            // 关闭 query（内部会释放全部 counter）
            let rc = unsafe { PdhCloseQuery(hq) };
            outln!(out, "  [close] B(expanded active) → {}", rcs(rc));
        }
    }

    // ---- Phase C：全部实例逐个添加（分离「实例数」与「counter 数」两个因子）----
    outln!(out, "");
    outln!(
        out,
        "== Phase C: 展开后的 {} 个实例全部逐个 PdhAddEnglishCounterW（分离 counter 数因子）==",
        all_paths.len()
    );
    if all_paths.is_empty() {
        outln!(out, "  [skip] 无展开结果");
    } else {
        let mut hq: isize = 0;
        let rc = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut hq) };
        if rc != 0 {
            outln!(out, "[FAIL] PdhOpenQueryW → {}", rcs(rc));
        } else {
            let mut counters: Vec<isize> = Vec::with_capacity(all_paths.len());
            let mut fail = 0usize;
            let t_add = Instant::now();
            for p in &all_paths {
                let w = wide(p);
                let mut hc: isize = 0;
                let rc = unsafe {
                    PdhAddEnglishCounterW(hq, PCWSTR::from_raw(w.as_ptr()), 0, &mut hc)
                };
                if rc != 0 {
                    fail += 1;
                } else {
                    counters.push(hc);
                }
            }
            let add_dur = t_add.elapsed();
            outln!(
                out,
                "  添加 {} 个 counter: 成功 {} 失败 {} ，总 {:.3} ms ，均值 {:.4} ms/个",
                all_paths.len(),
                counters.len(),
                fail,
                ms(add_dur),
                ms(add_dur) / all_paths.len().max(1) as f64
            );
            let qc = PdhQuery {
                label: "C(all instances, N counters)".to_string(),
                path: "<全部实例各自一个 counter>".to_string(),
                hquery: hq,
                counters,
            };
            let (rc1, d1) = qc.collect();
            outln!(
                out,
                "  baseline PdhCollectQueryData → {} , {:.3} ms",
                rcs(rc1),
                ms(d1)
            );
            let rows_c = run_ticks(&qc, 3, TICK_SLEEP_MS, PDH_FMT_DOUBLE.0, false);
            print_ticks(&mut out, "  Phase C 逐 tick（3 ticks，仅为成本量级）:", &rows_c);
            print_phase_summary(&mut out, "Phase C 统计", &rows_c);
            // 逐个 PdhRemoveCounter 的成本
            let t_rm = Instant::now();
            let mut rm_fail = 0usize;
            for hc in &qc.counters {
                let rc = unsafe { PdhRemoveCounter(*hc) };
                if rc != 0 {
                    rm_fail += 1;
                }
            }
            let rm_dur = t_rm.elapsed();
            outln!(
                out,
                "  PdhRemoveCounter × {}: 失败 {} ，总 {:.3} ms ，均值 {:.4} ms/个",
                qc.counters.len(),
                rm_fail,
                ms(rm_dur),
                ms(rm_dur) / qc.counters.len().max(1) as f64
            );
            let rc = unsafe { PdhCloseQuery(hq) };
            outln!(out, "  [close] C → {}", rcs(rc));
        }
    }

    // ---- 对照：GPU Process Memory(47) / GPU Adapter Memory(3) ----
    outln!(out, "");
    outln!(out, "== 对照 query：实例数 3 / 47（同一代码路径，观察成本是否随实例数增长）==");
    outln!(out, "  注: 对照 query 的 sum%/max% 列无意义（这两个计数器是内存字节数，且未解析实例名），只看 items 与耗时列");
    let mut ctrl: Vec<(&str, Vec<TickRow>)> = Vec::new();
    for (label, path) in [
        ("proc-mem(≈47)", PATH_PROC_MEM),
        ("adapter-mem(≈3)", PATH_ADAPTER_MEM),
    ] {
        outln!(out, "  -- {label}: {path}");
        match PdhQuery::open(label, path, &mut out) {
            Some(q) => {
                let (rc1, d1) = q.collect();
                outln!(
                    out,
                    "  baseline PdhCollectQueryData → {} , {:.3} ms",
                    rcs(rc1),
                    ms(d1)
                );
                let rows = run_ticks(&q, 6, TICK_SLEEP_MS, PDH_FMT_DOUBLE.0, false);
                print_ticks(&mut out, "  逐 tick:", &rows);
                print_phase_summary(&mut out, "  统计", &rows);
                ctrl.push((label, rows));
                q.close(&mut out);
            }
            None => outln!(out, "  [FAIL] {label} 无法建立（错误码见上）"),
        }
    }

    // ---- 方案对比总表 ----
    outln!(out, "");
    outln!(out, "== 方案对比与预算（核心结论区）==");
    let sa = phase_stats(&rows_a);
    let sb = phase_stats(&rows_b);
    let a_items = mean(&sa.items);
    let b_items = mean(&sb.items);
    outln!(
        out,
        "  (a) 通配持久 query      : counter=1  items≈{a_items:.0}  collect {:.3} + array {:.3} + parse {:.3} = {:.3} ms/tick",
        mean(&sa.collect_ms),
        mean(&sa.read_ms),
        mean(&sa.parse_ms),
        mean(&sa.total_ms)
    );
    if !rows_b.is_empty() {
        outln!(
            out,
            "  (b) 展开活跃实例        : counter={}  items≈{b_items:.0}  collect {:.3} + array {:.3} + parse {:.3} = {:.3} ms/tick",
            sel_paths.len().min(ACTIVE_CAP),
            mean(&sb.collect_ms),
            mean(&sb.read_ms),
            mean(&sb.parse_ms),
            mean(&sb.total_ms)
        );
        outln!(
            out,
            "  (b) 附加成本            : re-expand {:.3} ms/次（均值）+ add {:.4} ms/实例 × {} = {:.3} ms/轮",
            mean(&exp_times),
            add_avg_ms,
            sel_paths.len(),
            mean(&exp_times) + add_avg_ms * sel_paths.len() as f64
        );
        let ratio = if mean(&sa.total_ms) > 0.0 {
            mean(&sb.total_ms) / mean(&sa.total_ms)
        } else {
            f64::NAN
        };
        outln!(out, "  比值 (b)/(a) per-tick    : {ratio:.3}x");
    } else {
        outln!(out, "  (b) 展开活跃实例        : 未测得（活跃实例为 0）");
    }
    for (label, rows) in &ctrl {
        let s = phase_stats(rows);
        outln!(
            out,
            "  对照 {label:<16}: counter=1  items≈{:.0}  collect {:.3} + array {:.3} = {:.3} ms/tick",
            mean(&s.items),
            mean(&s.collect_ms),
            mean(&s.read_ms),
            mean(&s.total_ms)
        );
    }
    let a_total = mean(&sa.total_ms);
    outln!(
        out,
        "  1s 采样周期下 (a) 占该线程 {:.2}% 时间；2s 周期下 {:.2}%",
        a_total / 1000.0 * 100.0,
        a_total / 2000.0 * 100.0
    );
    // 成本是否随实例数线性：用同路径的 3 / 47 / 688 三点做粗估
    let mut pts: Vec<(f64, f64)> = vec![(a_items, mean(&sa.read_ms))];
    for (_, rows) in &ctrl {
        let s = phase_stats(rows);
        pts.push((mean(&s.items), mean(&s.read_ms)));
    }
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    outln!(
        out,
        "  实例数 vs array 时间点集: {}",
        pts.iter()
            .map(|(n, t)| format!("({n:.0} items, {t:.3} ms)"))
            .collect::<Vec<_>>()
            .join("  ")
    );
    if pts.len() >= 2 {
        let (n0, t0) = pts[0];
        let (n1, t1) = pts[pts.len() - 1];
        if n1 > n0 {
            let slope = (t1 - t0) / (n1 - n0);
            outln!(
                out,
                "  边际成本粗估: items {n0:.0}→{n1:.0} 时 array 时间 {t0:.3}→{t1:.3} ms → 约 {:.3} µs/item（线性拟合截距 {:.3} ms）",
                slope * 1000.0,
                t0 - slope * n0
            );
        }
    }

    // ---- 空转（无 sleep）成本 ----
    outln!(out, "");
    outln!(out, "== 无 sleep 连续 10 次（纯成本；间隔≈0 时 rate 计数器可能返回 INVALID_DATA，属预期）==");
    let rows_hot = run_ticks(&qa, 10, 0, PDH_FMT_DOUBLE.0, true);
    print_ticks(&mut out, "  逐 tick:", &rows_hot);
    print_phase_summary(&mut out, "  统计", &rows_hot);
    let hot_rc_bad = rows_hot
        .iter()
        .filter(|r| r.rc_collect != 0 || r.rc_read != 0)
        .count();
    outln!(
        out,
        "  10 次连续采样中带非 0 返回码的 tick: {} / 10",
        hot_rc_bad
    );

    // ---- 聚合报告（Phase A 数据）----
    outln!(out, "");
    outln!(out, "== 聚合（基于 Phase A 的 {TICKS} 个 tick）==");
    let n_ticks = rows_a.len().max(1) as f64;

    // engtype 直方图
    let mut hist_inst: BTreeMap<String, usize> = BTreeMap::new();
    let mut hist_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut hist_max: BTreeMap<String, f64> = BTreeMap::new();
    let mut raw_types: BTreeSet<String> = BTreeSet::new();
    for r in &rows_a {
        for (i, v) in &r.values {
            *hist_inst.entry(i.engtype.clone()).or_insert(0) += 1;
            *hist_sum.entry(i.engtype.clone()).or_insert(0.0) += *v;
            let e = hist_max.entry(i.engtype.clone()).or_insert(0.0);
            if *v > *e {
                *e = *v;
            }
            raw_types.insert(i.engtype_raw.clone());
        }
    }
    outln!(out, "  -- engine type 直方图（{TICKS} ticks 累计）--");
    outln!(
        out,
        "  {:<16} {:>10} {:>14} {:>12}",
        "engtype",
        "instances",
        "sum%",
        "max单实例%"
    );
    for (k, n) in &hist_inst {
        outln!(
            out,
            "  {:<16} {:>10} {:>14.2} {:>12.2}",
            k,
            n,
            hist_sum.get(k).copied().unwrap_or(0.0),
            hist_max.get(k).copied().unwrap_or(0.0)
        );
    }
    outln!(
        out,
        "  原始 engtype 字符串: {:?}",
        raw_types.iter().cloned().collect::<Vec<_>>()
    );

    // 按 PID 聚合
    #[derive(Default, Clone)]
    struct PidAgg {
        sum_total: f64,
        max_engine: f64,
        present: usize,
        by_type: BTreeMap<String, f64>,
        inst: usize,
        last_sum: f64,
    }
    let mut pids: BTreeMap<u32, PidAgg> = BTreeMap::new();
    for (ti, r) in rows_a.iter().enumerate() {
        let mut per: BTreeMap<u32, (f64, f64, BTreeMap<String, f64>, usize)> = BTreeMap::new();
        for (i, v) in &r.values {
            let e = per.entry(i.pid).or_insert((0.0, 0.0, BTreeMap::new(), 0));
            e.0 += *v;
            if *v > e.1 {
                e.1 = *v;
            }
            *e.2.entry(i.engtype.clone()).or_insert(0.0) += *v;
            e.3 += 1;
        }
        for (pid, (sum, mx, by, inst)) in per {
            let a = pids.entry(pid).or_default();
            a.sum_total += sum;
            if mx > a.max_engine {
                a.max_engine = mx;
            }
            a.present += 1;
            a.inst += inst;
            for (k, v) in by {
                *a.by_type.entry(k).or_insert(0.0) += v;
            }
            if ti == rows_a.len() - 1 {
                a.last_sum = sum;
            }
        }
    }
    let mut ranked: Vec<(u32, PidAgg)> = pids.iter().map(|(k, v)| (*k, v.clone())).collect();
    ranked.sort_by(|a, b| b.1.sum_total.partial_cmp(&a.1.sum_total).unwrap_or(std::cmp::Ordering::Equal));
    outln!(out, "  -- Top 15 进程（avg% = 窗口内均值；max单引擎%；present=出现 tick 数）--");
    outln!(
        out,
        "  {:>7} {:<28} {:>9} {:>11} {:>8} {:>7}  {}",
        "pid",
        "name",
        "avg%",
        "max引擎%",
        "present",
        "引擎数",
        "engtype 分解"
    );
    for (pid, a) in ranked.iter().take(15) {
        let nm = names
            .get(pid)
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| "<unknown/exited>".to_string());
        let mut by: Vec<(String, f64)> = a.by_type.iter().map(|(k, v)| (k.clone(), *v)).collect();
        by.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        let bys = by
            .iter()
            .map(|(k, v)| format!("{k} {:.0}%", v / n_ticks))
            .collect::<Vec<_>>()
            .join(", ");
        outln!(
            out,
            "  {:>7} {:<28} {:>9.2} {:>11.2} {:>8} {:>7}  {}",
            pid,
            nm,
            a.sum_total / n_ticks,
            a.max_engine,
            a.present,
            a.inst,
            bys
        );
    }
    let nonzero: usize = ranked
        .iter()
        .filter(|(_, a)| a.sum_total / n_ticks >= 0.01)
        .count();
    outln!(
        out,
        "  窗口内 avg% ≥0.01 的进程: {} / {}（总 PID 数）；合计 avg 求和 {:.1}%（跨引擎求和口径）",
        nonzero,
        ranked.len(),
        ranked.iter().map(|(_, a)| a.sum_total).sum::<f64>() / n_ticks
    );

    // 按 LUID 聚合
    outln!(out, "");
    outln!(out, "  -- 按 LUID（显卡）聚合 --");
    #[derive(Default, Clone)]
    struct LuidAgg {
        sum_total: f64,
        max_engine: f64,
        present: usize,
        pids: BTreeSet<u32>,
        inst: usize,
    }
    let mut luids: BTreeMap<(u32, u32), LuidAgg> = BTreeMap::new();
    for r in &rows_a {
        let mut per: BTreeMap<(u32, u32), (f64, f64, BTreeSet<u32>, usize)> = BTreeMap::new();
        for (i, v) in &r.values {
            let e = per
                .entry((i.luid_hi, i.luid_lo))
                .or_insert((0.0, 0.0, BTreeSet::new(), 0));
            e.0 += *v;
            if *v > e.1 {
                e.1 = *v;
            }
            e.2.insert(i.pid);
            e.3 += 1;
        }
        for (k, (sum, mx, ps, inst)) in per {
            let a = luids.entry(k).or_default();
            a.sum_total += sum;
            if mx > a.max_engine {
                a.max_engine = mx;
            }
            a.present += 1;
            a.inst += inst;
            for p in ps {
                a.pids.insert(p);
            }
        }
    }
    outln!(
        out,
        "  {:>10} {:>10}  {:<34} {:>9} {:>11} {:>8} {:>7}",
        "luid_hi",
        "luid_lo",
        "adapter(DXGI 匹配)",
        "avg%",
        "max引擎%",
        "present",
        "pids"
    );
    for ((hi, lo), a) in &luids {
        // 用两种字节序解释实例名里的 LUID，经验判定哪个与 DXGI 一致
        let m_same = adapters
            .iter()
            .find(|ad| ad.luid_hi == *hi && ad.luid_lo == *lo);
        let m_rev = adapters
            .iter()
            .find(|ad| ad.luid_hi == *lo && ad.luid_lo == *hi);
        let desc = match (m_same, m_rev) {
            (Some(ad), _) => format!("{} [按(hi,lo)匹配]", ad.name),
            (None, Some(ad)) => format!("{} [需反序(lo,hi)匹配!]", ad.name),
            (None, None) => "<DXGI 未列出（虚拟/软件适配器？）>".to_string(),
        };
        outln!(
            out,
            "  0x{:08X} 0x{:08X}  {:<34} {:>9.2} {:>11.2} {:>8} {:>7}",
            hi,
            lo,
            desc,
            a.sum_total / n_ticks,
            a.max_engine,
            a.present,
            a.pids.len()
        );
    }
    outln!(
        out,
        "  LUID 匹配自检: {} 个观测 LUID 中，按 (hi,lo) 直接匹配 DXGI 的 {} 个，需反序匹配的 {} 个，未匹配 {} 个",
        luids.len(),
        luids
            .keys()
            .filter(|(hi, lo)| adapters.iter().any(|a| a.luid_hi == *hi && a.luid_lo == *lo))
            .count(),
        luids
            .keys()
            .filter(|(hi, lo)| {
                adapters.iter().any(|a| a.luid_hi == *lo && a.luid_lo == *hi)
                    && !adapters.iter().any(|a| a.luid_hi == *hi && a.luid_lo == *lo)
            })
            .count(),
        luids
            .keys()
            .filter(|(hi, lo)| {
                !adapters.iter().any(|a| a.luid_hi == *hi && a.luid_lo == *lo)
                    && !adapters.iter().any(|a| a.luid_hi == *lo && a.luid_lo == *hi)
            })
            .count()
    );

    // ---- 退化情况汇总 ----
    outln!(out, "");
    outln!(out, "== 退化情况与正确性注意事项 ==");
    let zero_inst = rows_a.iter().all(|r| r.items == 0);
    outln!(
        out,
        "  * 0 实例: {}",
        if zero_inst {
            "本次运行全程 0 实例（无 GPU 活动）→ 已按空集处理，不 panic"
        } else {
            "未出现（窗口内始终有实例）"
        }
    );
    let inv = rows_a
        .iter()
        .filter(|r| {
            r.rc_collect == 0xC000_0BBA
                || r.rc_read == 0xC000_0BBA
                || r.rc_read == 0xC000_0BC6
                || r.rc_collect == 0xC000_0BC6
        })
        .count();
    let noinst = rows_a
        .iter()
        .filter(|r| r.rc_read == 0x8000_07D1 || r.rc_read == 0x8000_07D5)
        .count();
    outln!(
        out,
        "  * PDH_CSTATUS_INVALID_DATA/PDH_INVALID_DATA tick 数: {inv}/{TICKS}；PDH_CSTATUS_NO_INSTANCE/PDH_NO_DATA tick 数: {noinst}/{TICKS}"
    );
    let baditem_total: usize = rows_a.iter().map(|r| r.bad_items).sum();
    outln!(
        out,
        "  * 单项 CStatus 非 0/1 的实例数累计: {baditem_total}（已跳过并计数，不参与聚合）"
    );
    outln!(
        out,
        "  * 跨引擎求和口径: 单 PID 求和可 >100%（本次最大 {max_pid_sum:.2}%）；不做 100 封顶"
    );
    outln!(
        out,
        "  * rate 计数器需要两次 collect 才有值；首次读取（单次 collect）的返回码已在上文逐字记录"
    );
    outln!(
        out,
        "  * 进程名是独立快照（NtQuerySystemInformation），与 GPU 采样存在毫秒级时间差；已退出进程显示 <unknown/exited>"
    );
    outln!(
        out,
        "  * 方案 (b) 的活跃实例集合来自 Phase A 最后一个 tick，Phase B(b) 期间发生 churn 的实例会返回 NO_INSTANCE（见 badcnt 列）"
    );

    finalize(out, out_arg, t_all);
}

fn finalize(mut out: Out, out_arg: Option<String>, t_all: Instant) {
    outln!(out, "");
    outln!(out, "== 运行总计 ==");
    outln!(out, "  进程总 wall time: {:.2} s", t_all.elapsed().as_secs_f64());
    let cpu = proc_cpu_100ns();
    if cpu >= 0 {
        outln!(
            out,
            "  本进程累计 CPU (kernel+user): {:.3} s （占单核 {:.1}%）",
            cpu as f64 / 1e7,
            cpu as f64 / 1e7 / t_all.elapsed().as_secs_f64() * 100.0
        );
    }
    if let Some(p) = out_arg {
        if let Some(dir) = std::path::Path::new(&p).parent() {
            if !dir.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("[FAIL] create_dir_all({}) → {e}", dir.display());
                }
            }
        }
        let write_res = std::fs::write(&p, &out.buf);
        let n_bytes = out.buf.len();
        let n_lines = out.buf.lines().count();
        match write_res {
            Ok(()) => outln!(out, "  原始输出已写入: {p} ({n_bytes} bytes, {n_lines} 行)"),
            Err(e) => {
                eprintln!("[FAIL] 写文件 {p} 失败: {e}");
                println!("[FAIL] 写文件 {p} 失败: {e}");
            }
        }
    }
}
