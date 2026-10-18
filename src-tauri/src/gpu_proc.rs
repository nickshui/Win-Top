//! 每进程 GPU 占用追踪：PDH `\GPU Engine(*)\Utilization Percentage`。
//!
//! 结构与 `disk_io.rs` 同构：后台线程 → 每 2 秒 `app.emit_all("gpu-proc", payload)`，
//! 前端无需轮询、也不需要新增 command。
//!
//! 采集口径 = PoC 已实测的「方案 (a)」：**一个**长生命周期的查询句柄 + **一个**
//! 通配符 counter，每 tick 只调用一次 `PdhGetFormattedCounterArrayW` 读回全部实例
//! （本机 688 个实例 / 29 个 PID / 3 个 LUID，实测 1.9 ms 每 tick）。
//! 绝不按实例逐个 `PdhAddEnglishCounterW`（PoC 实测那样慢 6.5 倍）。
//!
//! 依据：`docs/research/2026-10-18-gpu-integration-plan.md` §4，
//! 实测原始数据 `docs/research/evidence/2026-10-18-gpu-pdh-poc.txt`。
//! 全部走 Windows 原生接口，不派生任何子进程。

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use windows::core::{w, PCWSTR};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_NO_COUNTER, PDH_CSTATUS_NO_INSTANCE,
    PDH_CSTATUS_NO_OBJECT, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_MORE_DATA,
};

/// 采样周期：2 秒，与 `disk_io.rs` 及进程表刷新节奏一致。
const CADENCE: Duration = Duration::from_secs(2);
/// 首个快照前的等待。必须 ≥1 秒（PDH 速率计数器在 <1 秒的连续采样下返回码正常但数值静默为 0，
/// PoC 实测），这里取 1 秒而非 2 秒，只为让前端尽早拿到第一份快照，之后一律按 CADENCE。
const FIRST_DELAY: Duration = Duration::from_secs(1);
/// 计数器集不可用 / 运行中消失后的退避重试间隔，线程不退出。
const RETRY_BACKOFF: Duration = Duration::from_secs(60);
/// `rows` 截断条数。
const TOP_N: usize = 128;
/// 每行携带的引擎明细条数。
const TOP_ENGINES: usize = 8;
/// 每适配器携带的引擎分解条数（按 pct 降序）。
const TOP_ADAPTER_ENGINES: usize = 4;
/// 每进程携带的逐卡分解条数（按 gpu_pct 降序）。
const TOP_PER_ADAPTER: usize = 4;
/// 历史发布槽里 `adapters` 的截断条数（按 gpu_pct 降序保留）。
const HIST_ADAPTERS: usize = 8;

/// 显存辅助 counter。**必须与引擎 counter 在同一个 query 里、由同一次
/// `PdhCollectQueryData` 刷新**：task-11 实测三次独立 `Get-Counter` 读数彼此不自洽
/// （独显 Dedicated + Shared ≠ Total Committed），跨调用读会拿到互相矛盾的数。
const PATH_ADAPTER_DEDICATED: &str = "\\GPU Adapter Memory(*)\\Dedicated Usage";
const PATH_ADAPTER_SHARED: &str = "\\GPU Adapter Memory(*)\\Shared Usage";
const PATH_PROC_DEDICATED: &str = "\\GPU Process Memory(*)\\Dedicated Usage";
const PATH_PROC_SHARED: &str = "\\GPU Process Memory(*)\\Shared Usage";
/// 空 engtype（Intel 核显实测有 `..._engtype_` 实例）在分解里的显示名。
const EMPTY_ENGTYPE_LABEL: &str = "(空)";
/// MB = 1024²，与 `process.rs` 的 `mem_mb` 口径一致。
const MB: f64 = 1024.0 * 1024.0;

static STARTED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// 冻结的数据契约（字段名与前端约定逐一对应，不得改动）
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GpuEngineRow {
    pub eng: u32,
    pub engtype: String,
    pub pct: f64,
}

/// 进程在某一张卡上的分解条目（供详情面板回答"用哪张卡、各占多少"）。
/// 不放 `name`：前端用同一 tick 的 `adapters[]` 做 luid→name 映射即可，payload 更小。
#[derive(Serialize, Clone, Debug)]
pub struct GpuProcAdapterRow {
    pub luid: String,
    /// 该进程在该卡上的最忙引擎占用（跨该卡 `(luid, eng)` 合并后取 max），0~100。
    pub gpu_pct: f64,
    /// 该进程在该卡上的专用显存（MB，一位小数），按 (pid, luid) 精确匹配；无实例记 0。
    pub vram_dedicated_mb: f64,
    /// 该进程在该卡上的共享显存（MB，一位小数）。
    pub vram_shared_mb: f64,
}

#[derive(Serialize, Clone)]
pub struct GpuProcRow {
    pub pid: u32,
    pub name: String,
    /// 最忙引擎占用（Task Manager 口径）：先在 `(luid, phys, eng)` 上合并，再跨 luid 取 max。
    pub gpu_pct: f64,
    /// 该 PID 在**胜出适配器**上的各引擎求和，可能 >100（仅作次级指标）。
    pub gpu_sum_pct: f64,
    /// 胜出引擎所属适配器，形如 `0x00000000:0x0000D969`。
    pub luid: String,
    /// 胜出引擎的原始 engtype 标签（自带序号，仅作显示，不作为稳定标识）。
    pub engtype: String,
    /// `\GPU Process Memory(*)\Dedicated Usage`（MB，1024²）。取不到记 0，**不因此剔除该行**。
    pub vram_dedicated_mb: f64,
    /// `\GPU Process Memory(*)\Shared Usage`（MB，1024²）。取不到记 0。
    pub vram_shared_mb: f64,
    pub engines: Vec<GpuEngineRow>,
    /// 逐卡分解：只含该进程**真正有实例**的卡（Engine ∪ Process Memory），最多 [`TOP_PER_ADAPTER`] 条。
    /// 汇总字段（`gpu_pct`/`gpu_sum_pct`/`vram_*_mb`）是跨卡口径，这里是分解口径，两者并存。
    pub per_adapter: Vec<GpuProcAdapterRow>,
}

/// 适配器上的引擎分解条目：对 `(luid, engtype)` 跨 PID 求和后的合计占用。
#[derive(Serialize, Clone, Debug)]
pub struct GpuAdapterEngineRow {
    pub engtype: String,
    pub pct: f64,
}

#[derive(Serialize, Clone)]
pub struct GpuAdapterRow {
    pub luid: String,
    /// DXGI `Description`（trim 后）；DXGI 失败或该 LUID 未匹配时兜底为 `Adapter <LO>`，永不空串。
    pub name: String,
    /// `hardware` / `software` / `virtual`（判定见 [`classify_adapter`]）。
    pub kind: String,
    pub gpu_pct: f64,
    /// `\GPU Adapter Memory(*)\Dedicated Usage`（字节）。该 LUID 取不到记 0（核显实测为 0）。
    pub vram_dedicated_bytes: u64,
    /// `\GPU Adapter Memory(*)\Shared Usage`（字节）。该 LUID 取不到记 0。
    pub vram_shared_bytes: u64,
    /// DXGI `DedicatedVideoMemory`（字节）；virtual / DXGI 映射失败记 0。**不是** Usage。
    pub vram_capacity_bytes: u64,
    /// 该适配器按 engtype 跨 PID 求和的引擎分解，降序前 [`TOP_ADAPTER_ENGINES`] 条，不封顶。
    pub engines: Vec<GpuAdapterEngineRow>,
}

#[derive(Serialize, Clone)]
pub struct GpuProcSnapshot {
    pub ts: String,
    pub available: bool,
    pub reason: String,
    /// 本 tick 的采集 + 解析 + 聚合耗时（毫秒，不含 sleep，也不含进程名枚举）。
    pub cost_ms: f64,
    pub adapters: Vec<GpuAdapterRow>,
    pub rows: Vec<GpuProcRow>,
}

// ---------------------------------------------------------------------------
// 历史发布槽（冻结接口，collector 每秒调用一次 histogram_sample() 写入历史）
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize)]
pub struct GpuHistAdapter {
    pub luid: String,
    pub name: String,
    pub gpu_pct: f64,
}

#[derive(Clone, Default, Serialize)]
pub struct GpuHistSample {
    /// 计数器可用且本 tick 产出了快照（GPU 完全空闲时 adapters 为空，但仍是有效读数）。
    pub available: bool,
    /// 跨适配器 max（绝不求和），与 `gpu-proc` payload 的胜出者一致。
    pub gpu_pct: f64,
    /// 胜出适配器 luid；无适配器时为空串。
    pub luid: String,
    /// 全部适配器（含 virtual），按 gpu_pct 降序，最多 [`HIST_ADAPTERS`] 条。
    pub adapters: Vec<GpuHistAdapter>,
}

/// 发布槽。未就绪（尚未采到第一个 tick / 计数器不可用）时保持 `Default`。
static HIST: LazyLock<Mutex<GpuHistSample>> = LazyLock::new(|| Mutex::new(GpuHistSample::default()));

/// 供 collector 每秒写入历史用。未就绪时返回 `Default`；只 clone 一个小结构，廉价。
pub fn histogram_sample() -> GpuHistSample {
    HIST.lock().map(|g| g.clone()).unwrap_or_default()
}

/// 每 tick 聚合完成后更新一次发布槽。胜出者取 `adapters[0]`（已按 gpu_pct 降序）。
fn publish_histogram(snapshot: &GpuProcSnapshot) {
    let mut hist = GpuHistSample {
        available: snapshot.available,
        ..Default::default()
    };
    if snapshot.available {
        if let Some(top) = snapshot.adapters.first() {
            hist.gpu_pct = top.gpu_pct;
            hist.luid = top.luid.clone();
        }
        hist.adapters = snapshot
            .adapters
            .iter()
            .take(HIST_ADAPTERS)
            .map(|a| GpuHistAdapter {
                luid: a.luid.clone(),
                name: a.name.clone(),
                gpu_pct: a.gpu_pct,
            })
            .collect();
    }
    if let Ok(mut slot) = HIST.lock() {
        *slot = hist;
    }
}

/// 计数器不可用 / 运行中消失时把发布槽复位为 Default（collector 会看到 available=false）。
fn reset_histogram() {
    if let Ok(mut slot) = HIST.lock() {
        *slot = GpuHistSample::default();
    }
}

// ---------------------------------------------------------------------------
// DXGI 适配器枚举（windows crate 生成绑定，只枚举一次并缓存）
//
// 依赖 `src-tauri/Cargo.toml` 里 windows 0.58 的 feature `Win32_Graphics_Dxgi`。
// 实测 `CreateDXGIFactory1::<IDXGIFactory1>()` 不需要 CoInitializeEx（PoC / task-11 两次验证）。
// 任何失败都只记录原因并降级为「兜底名字 + virtual」，绝不 panic、绝不让 tick 失败。
// ---------------------------------------------------------------------------

/// `DXGI_ADAPTER_FLAG_SOFTWARE` 的 u32 形式：`DXGI_ADAPTER_DESC1::Flags` 是 `u32`，
/// 而生成常量是 `DXGI_ADAPTER_FLAG(i32)` newtype，比较前统一到 u32。
const ADAPTER_FLAG_SOFTWARE_U32: u32 = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;

struct DxgiAdapterInfo {
    /// 已 trim 尾部 NUL 与空白；可能为空串（此时调用方走兜底名字）。
    name: String,
    flags: u32,
    /// `DXGI_ADAPTER_DESC1::DedicatedVideoMemory`（字节）→ `vram_capacity_bytes`。
    dedicated_video_memory: usize,
}

/// `Description` 是定长 `[u16; 128]`：截到第一个 NUL，再 trim 空白。
fn trim_utf16_desc(desc: &[u16; 128]) -> String {
    let end = desc.iter().position(|&c| c == 0).unwrap_or(desc.len());
    String::from_utf16_lossy(&desc[..end]).trim().to_string()
}

/// 名字兜底：`Adapter 0000D969`。规格要求 name 永不为空串。
fn fallback_adapter_name(luid_lo: u32) -> String {
    format!("Adapter {:08X}", luid_lo)
}

/// 枚举 DXGI 适配器一次；返回 (适配器列表, 失败/中断原因)。任何路径都不 panic。
fn enumerate_dxgi_adapters() -> (Vec<(u32, u32, DxgiAdapterInfo)>, Option<String>) {
    let mut out: Vec<(u32, u32, DxgiAdapterInfo)> = Vec::new();
    let mut err: Option<String> = None;
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        Ok(f) => f,
        Err(e) => {
            return (
                out,
                Some(format!(
                    "CreateDXGIFactory1(IDXGIFactory1) 失败 0x{:08X} {:?}",
                    e.code().0 as u32,
                    e
                )),
            );
        }
    };

    let mut index: u32 = 0;
    loop {
        match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => {
                match unsafe { adapter.GetDesc1() } {
                    Ok(desc) => out.push((
                        desc.AdapterLuid.HighPart as u32,
                        desc.AdapterLuid.LowPart,
                        DxgiAdapterInfo {
                            name: trim_utf16_desc(&desc.Description),
                            flags: desc.Flags,
                            dedicated_video_memory: desc.DedicatedVideoMemory,
                        },
                    )),
                    Err(e) => {
                        if err.is_none() {
                            err = Some(format!(
                                "GetDesc1({index}) 失败 0x{:08X} {:?}",
                                e.code().0 as u32,
                                e
                            ));
                        }
                    }
                }
                // `adapter` 是 windows crate 的 COM 包装，离开作用域自动 Release
            }
            Err(e) => {
                // DXGI_ERROR_NOT_FOUND = 枚举结束的正常返回，不是错误
                if e.code() != DXGI_ERROR_NOT_FOUND {
                    err = Some(format!(
                        "EnumAdapters1({index}) 失败 0x{:08X} {:?}",
                        e.code().0 as u32,
                        e
                    ));
                }
                break;
            }
        }
        index += 1;
        if index > 64 {
            err = Some("适配器数超过 64，停止枚举".to_string());
            break;
        }
    }
    (out, err)
}

/// 进程生命周期内适配器不会变：首次用到时枚举一次并缓存（失败也只尝试一次）。
static DXGI_ADAPTERS: LazyLock<HashMap<(u32, u32), DxgiAdapterInfo>> = LazyLock::new(|| {
    let (list, err) = enumerate_dxgi_adapters();
    if let Some(e) = &err {
        eprintln!(
            "[gpu_proc] DXGI 适配器枚举未完整成功：{e}；未匹配到的 LUID 按 virtual + 兜底名字处理"
        );
    }
    list.into_iter().map(|(hi, lo, info)| ((hi, lo), info)).collect()
});

/// kind 三分支判定 + 名字兜底（纯函数，单测可注入 DXGI 映射）：
/// 1. 按 `(HighPart, LowPart)` **原序**匹配到且 `Flags & 0x2 == 0` → `hardware`
/// 2. 匹配到且 `Flags & 0x2 != 0`（`DXGI_ADAPTER_FLAG_SOFTWARE`）→ `software`
/// 3. 未匹配到（含 DXGI 整体失败、映射为空）→ `virtual`
///
/// `name` 永不为空：匹配到但 Description 为空时同样兜底为 `Adapter <LO>`（kind 仍按 flags 判定）。
fn classify_adapter_in(
    luid_hi: u32,
    luid_lo: u32,
    dxgi: &HashMap<(u32, u32), DxgiAdapterInfo>,
) -> (&'static str, String) {
    let kind_of = |flags: u32| {
        if flags & ADAPTER_FLAG_SOFTWARE_U32 != 0 {
            "software"
        } else {
            "hardware"
        }
    };
    match dxgi.get(&(luid_hi, luid_lo)) {
        Some(a) if !a.name.is_empty() => (kind_of(a.flags), a.name.clone()),
        Some(a) => (kind_of(a.flags), fallback_adapter_name(luid_lo)),
        None => ("virtual", fallback_adapter_name(luid_lo)),
    }
}

// ---------------------------------------------------------------------------
// 实例名解析
// ---------------------------------------------------------------------------

/// 稳定引擎标识。`engtype` 带序号（`Compute 0/1/3`）且跨驱动版本不稳定，只作显示标签。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct EngineKey {
    luid_hi: u32,
    luid_lo: u32,
    phys: u32,
    eng: u32,
}

struct GpuInstance {
    pid: u32,
    luid_hi: u32,
    luid_lo: u32,
    phys: u32,
    eng: u32,
    engtype: String,
}

fn luid_key(hi: u32, lo: u32) -> String {
    format!("0x{:08X}:0x{:08X}", hi, lo)
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    let t = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(t, 16).ok()
}

/// `\GPU Adapter Memory(*)` 的实例名：`luid_0x00000000_0x0000d969_phys_0`
/// （**没有 pid 段**，实测 hex 大小写都可能出现）→ `(0x00000000, 0x0000d969)`。
/// 解析失败返回 `None`，由调用方跳过该项，绝不 panic。
fn parse_luid_instance(name: &str) -> Option<(u32, u32)> {
    let rest = name.strip_prefix("luid_")?;
    let (hi_s, tail) = rest.split_once('_')?;
    let luid_hi = parse_hex_u32(hi_s)?;
    let lo_end = tail.find("_phys_").unwrap_or(tail.len());
    let luid_lo = parse_hex_u32(&tail[..lo_end])?;
    Some((luid_hi, luid_lo))
}

/// `pid_10668_luid_0x00000000_0x0000D969_phys_0_eng_0_engtype_High Priority 3D`
/// → pid=10668, luid=0x00000000:0x0000D969, phys=0, eng=0, engtype="High Priority 3D"。
///
/// 规则（PoC 实测 688/688 解析成功）：
/// - `engtype_` 之后**贪婪取到字符串末尾**（可含空格），**可以为空字符串**；
/// - hex 真实写法是大写（`0x0000D969`），但这里大小写都接受；
/// - 某些驱动的简化形式没有 `_phys_` / `_eng_`，缺失时按 0 处理；
/// - 任何字段缺失/非法都返回 `None`，由调用方计数跳过，绝不 panic。
fn parse_instance(name: &str) -> Option<GpuInstance> {
    let rest = name.strip_prefix("pid_")?;
    let (pid_s, rest) = rest.split_once("_luid_")?;
    let pid: u32 = pid_s.parse().ok()?;

    // rest = "0x00000000_0x0000D969_phys_0_eng_0_engtype_3D"
    let (head, engtype) = match rest.split_once("_engtype_") {
        Some((h, t)) => (h, t.to_string()),
        None => (rest, String::new()),
    };
    // head = "0x00000000_0x0000D969_phys_0_eng_0"
    let (hi_s, tail) = head.split_once('_')?;
    let luid_hi = parse_hex_u32(hi_s)?;
    let lo_end = tail
        .find("_phys_")
        .or_else(|| tail.find("_eng_"))
        .unwrap_or(tail.len());
    let luid_lo = parse_hex_u32(&tail[..lo_end])?;

    let num_after = |marker: &str| -> u32 {
        tail.split_once(marker)
            .and_then(|(_, t)| t.split('_').next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    };

    Some(GpuInstance {
        pid,
        luid_hi,
        luid_lo,
        phys: num_after("_phys_"),
        eng: num_after("_eng_"),
        engtype,
    })
}

// ---------------------------------------------------------------------------
// PDH 查询（句柄只开一次，缓冲跨 tick 复用）
// ---------------------------------------------------------------------------

/// 显存辅助 counter 的读取结果（同一 query、同一次 collect 刷新）。
#[derive(Default)]
struct MemSample {
    /// (luid_hi, luid_lo) → (Dedicated Usage, Shared Usage) 字节
    adapter: HashMap<(u32, u32), (f64, f64)>,
    /// (pid, luid_hi, luid_lo) → (Dedicated Usage, Shared Usage) 字节。
    /// 按 **(pid, luid)** 精确匹配：同一进程在多张卡上的实例必须分开存，不能互相覆盖。
    process: HashMap<(u32, u32, u32), (f64, f64)>,
    /// 读取失败的 (counter 路径, PDH 返回码)；非致命，对应字段按 0
    fail: Vec<(&'static str, u32)>,
}

/// 一次读取的原始结果（已按单项 CStatus 过滤）。
#[derive(Default)]
struct Sample {
    /// (实例, 该引擎的 Utilization Percentage)
    items: Vec<(GpuInstance, f64)>,
    /// PDH 返回的原始实例数（含被过滤掉的项）
    instances: u32,
    /// 单项 CStatus 非 VALID_DATA/NEW_DATA 而跳过的项数
    bad_items: u32,
    /// 实例名解析失败而跳过的项数
    parse_fail: u32,
    /// 显存（适配器 + 每进程），与引擎读数同一次 collect
    mem: MemSample,
}

/// 单个 counter 的原始数组读取结果。
#[derive(Default)]
struct RawRead {
    /// (实例名, 值)，已过滤掉单项 CStatus 无效的项
    items: Vec<(String, f64)>,
    /// PDH 报的实例数（含被过滤项）
    instances: u32,
    /// 单项 CStatus 非 VALID_DATA/NEW_DATA 而跳过的项数
    bad_items: u32,
    rc: u32,
}

struct GpuQuery {
    hquery: isize,
    /// `\GPU Engine(*)`：必需 counter，打开失败即整个查询不可用
    hcounter: isize,
    /// 显存辅助 counter；0 = 该 counter 打开失败（字段按 0 处理，不影响 tick）
    hadapter_dedicated: isize,
    hadapter_shared: isize,
    hproc_dedicated: isize,
    hproc_shared: isize,
    /// `PDH_FMT_COUNTERVALUE_ITEM_W` 需要 8 字节对齐，故用 `Vec<u64>` 承载并按需增长。
    /// 各 counter 顺序读取，共用这一个缓冲。
    buf: Vec<u64>,
}

/// 计数器集整体消失（驱动重启 / GPU 复位）而非单次读数失败。
fn is_counter_gone(rc: u32) -> bool {
    rc == PDH_CSTATUS_NO_OBJECT || rc == PDH_CSTATUS_NO_COUNTER
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 添加一个**可选** counter：失败只记录原因并返回 0（对应字段按 0，不影响 tick）。
unsafe fn add_optional_counter(
    hquery: isize,
    path: &'static str,
    warnings: &mut Vec<String>,
) -> isize {
    let mut h: isize = 0;
    let wpath = wide(path);
    let rc = PdhAddEnglishCounterW(hquery, PCWSTR::from_raw(wpath.as_ptr()), 0, &mut h);
    if rc != 0 {
        warnings.push(format!("{path} → 0x{rc:08X}"));
        0
    } else {
        h
    }
}

impl GpuQuery {
    /// 打开查询：引擎 counter 必需；4 个显存 counter 可选（失败降级为 0）。
    /// 失败返回人话原因（不含 panic）。
    fn open() -> Result<GpuQuery, String> {
        unsafe {
            let mut hquery: isize = 0;
            let rc = PdhOpenQueryW(None, 0, &mut hquery);
            if rc != 0 {
                return Err(format!(
                    "PDH 查询无法创建（PdhOpenQueryW 0x{:08X}），稍后自动重试",
                    rc
                ));
            }

            let mut hcounter: isize = 0;
            let rc = PdhAddEnglishCounterW(
                hquery,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut hcounter,
            );
            if rc != 0 {
                let _ = PdhCloseQuery(hquery);
                let hint = match rc {
                    PDH_CSTATUS_NO_OBJECT => "系统未提供 GPU Engine 性能计数器集",
                    PDH_CSTATUS_NO_COUNTER => {
                        "GPU Engine 计数器集存在，但没有 Utilization Percentage 计数器"
                    }
                    PDH_CSTATUS_NO_INSTANCE => "GPU Engine 计数器集当前没有实例",
                    _ => "GPU 性能计数器不可用",
                };
                return Err(format!(
                    "{}（PdhAddEnglishCounterW 0x{:08X}；虚拟机 / 远程桌面会话或较旧的显卡驱动常见），稍后自动重试",
                    hint, rc
                ));
            }

            // 显存 counter 与引擎 counter **同一个 query**：由同一次 PdhCollectQueryData 刷新，
            // 保证 Dedicated/Shared 与引擎读数来自同一时刻（task-11 实测跨调用独立读会自相矛盾）。
            let mut warnings: Vec<String> = Vec::new();
            let hadapter_dedicated =
                add_optional_counter(hquery, PATH_ADAPTER_DEDICATED, &mut warnings);
            let hadapter_shared = add_optional_counter(hquery, PATH_ADAPTER_SHARED, &mut warnings);
            let hproc_dedicated = add_optional_counter(hquery, PATH_PROC_DEDICATED, &mut warnings);
            let hproc_shared = add_optional_counter(hquery, PATH_PROC_SHARED, &mut warnings);
            if !warnings.is_empty() {
                eprintln!(
                    "[gpu_proc] 显存计数器不可用（对应字段按 0 处理，不影响 GPU 占用采集）：{}",
                    warnings.join("；")
                );
            }

            Ok(GpuQuery {
                hquery,
                hcounter,
                hadapter_dedicated,
                hadapter_shared,
                hproc_dedicated,
                hproc_shared,
                buf: Vec::new(),
            })
        }
    }

    /// 读取单个 counter 的全部实例（复用 `self.buf`）。
    /// 两次调用模式：先取所需 buffer 大小（`PDH_MORE_DATA` 属正常），再取数据；
    /// 实例数会抖动（PoC 实测 688→739），缓冲增长后重试一次，仍失败则返回 `rc`。
    fn read_raw(&mut self, hcounter: isize) -> RawRead {
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        let rc = unsafe {
            PdhGetFormattedCounterArrayW(hcounter, PDH_FMT_DOUBLE, &mut size, &mut count, None)
        };
        if rc == PDH_CSTATUS_NO_INSTANCE {
            // 完全没有实例 = 合法空集（GPU 完全空闲 / 该计数器当前无实例）
            return RawRead::default();
        }
        if rc != 0 && rc != PDH_MORE_DATA {
            return RawRead {
                rc,
                ..Default::default()
            };
        }
        if size == 0 || count == 0 {
            return RawRead::default();
        }

        for attempt in 0..2 {
            self.buf.clear();
            self.buf.resize(size.div_ceil(8) as usize, 0u64);

            let mut size2 = size;
            let mut count2 = count;
            let rc2 = unsafe {
                PdhGetFormattedCounterArrayW(
                    hcounter,
                    PDH_FMT_DOUBLE,
                    &mut size2,
                    &mut count2,
                    Some(self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W),
                )
            };
            if rc2 == PDH_MORE_DATA {
                if attempt == 0 {
                    // PDH 会回写所需尺寸；即使没回写也至少翻倍，保证缓冲能增长
                    size = if size2 > size { size2 } else { size * 2 };
                    count = count2.max(count);
                    continue;
                }
                return RawRead {
                    rc: rc2,
                    ..Default::default()
                };
            }
            if rc2 != 0 {
                return RawRead {
                    rc: rc2,
                    ..Default::default()
                };
            }

            let mut out = RawRead::default();
            let items = unsafe {
                std::slice::from_raw_parts(
                    self.buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                    count2 as usize,
                )
            };
            for it in items {
                out.instances += 1;
                let cs = it.FmtValue.CStatus;
                // PDH_CSTATUS_VALID_DATA(0) 与 PDH_CSTATUS_NEW_DATA(1) 都算有效；
                // 其余（实例在采样中途换入换出）跳过该项并计数，不让整个 tick 失败，
                // 也绝不把它当成 0（PoC 实测一个 tick 里有 15 个这种项）。
                if cs != PDH_CSTATUS_VALID_DATA && cs != PDH_CSTATUS_NEW_DATA {
                    out.bad_items += 1;
                    continue;
                }
                if it.szName.0.is_null() {
                    out.bad_items += 1;
                    continue;
                }
                let raw = unsafe { String::from_utf16_lossy(it.szName.as_wide()) };
                let value = unsafe { it.FmtValue.Anonymous.doubleValue };
                out.items.push((raw, value));
            }
            return out;
        }
        RawRead {
            rc: PDH_MORE_DATA,
            ..Default::default()
        }
    }

    /// 读一对适配器显存 counter，按 LUID 合并。失败的 counter 记入 `fail`，字段按 0。
    fn read_adapter_vram(
        &mut self,
        fail: &mut Vec<(&'static str, u32)>,
    ) -> HashMap<(u32, u32), (f64, f64)> {
        let mut map: HashMap<(u32, u32), (f64, f64)> = HashMap::new();
        for (path, handle, dedicated) in [
            (PATH_ADAPTER_DEDICATED, self.hadapter_dedicated, true),
            (PATH_ADAPTER_SHARED, self.hadapter_shared, false),
        ] {
            if handle == 0 {
                continue; // 打开时就失败了，已在 open() 里记录过
            }
            let r = self.read_raw(handle);
            if r.rc != 0 {
                fail.push((path, r.rc));
                continue;
            }
            for (name, v) in r.items {
                if let Some((hi, lo)) = parse_luid_instance(&name) {
                    let slot = map.entry((hi, lo)).or_insert((0.0, 0.0));
                    if dedicated {
                        slot.0 = v;
                    } else {
                        slot.1 = v;
                    }
                }
            }
        }
        map
    }

    /// 读一对每进程显存 counter，按 **(pid, luid)** 合并（多卡进程必须分开存）。
    fn read_process_vram(
        &mut self,
        fail: &mut Vec<(&'static str, u32)>,
    ) -> HashMap<(u32, u32, u32), (f64, f64)> {
        let mut map: HashMap<(u32, u32, u32), (f64, f64)> = HashMap::new();
        for (path, handle, dedicated) in [
            (PATH_PROC_DEDICATED, self.hproc_dedicated, true),
            (PATH_PROC_SHARED, self.hproc_shared, false),
        ] {
            if handle == 0 {
                continue;
            }
            let r = self.read_raw(handle);
            if r.rc != 0 {
                fail.push((path, r.rc));
                continue;
            }
            for (name, v) in r.items {
                // 实例名 `pid_<PID>_luid_<hi>_<lo>_phys_0`：pid 与 luid 都能解析出来
                if let Some(inst) = parse_instance(&name) {
                    let slot = map
                        .entry((inst.pid, inst.luid_hi, inst.luid_lo))
                        .or_insert((0.0, 0.0));
                    if dedicated {
                        slot.0 = v;
                    } else {
                        slot.1 = v;
                    }
                }
            }
        }
        map
    }

    /// 一次 collect 之后读取全部 counter：引擎（必需）+ 显存（可选）。
    fn read(&mut self) -> Result<Sample, u32> {
        let eng = self.read_raw(self.hcounter);
        if eng.rc != 0 {
            return Err(eng.rc);
        }

        let mut out = Sample {
            instances: eng.instances,
            bad_items: eng.bad_items,
            ..Default::default()
        };
        for (raw, value) in eng.items {
            match parse_instance(&raw) {
                Some(inst) => out.items.push((inst, value)),
                None => out.parse_fail += 1,
            }
        }

        let adapter = self.read_adapter_vram(&mut out.mem.fail);
        out.mem.adapter = adapter;
        let process = self.read_process_vram(&mut out.mem.fail);
        out.mem.process = process;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 聚合
// ---------------------------------------------------------------------------

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// PDH 的字节数（f64）→ u64；负值 / NaN / 无穷归 0（取不到就是 0，不报错）。
fn bytes_of(v: f64) -> u64 {
    if v.is_finite() && v > 0.0 {
        v as u64
    } else {
        0
    }
}

fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

fn unavailable(reason: &str) -> GpuProcSnapshot {
    GpuProcSnapshot {
        ts: now_hms(),
        available: false,
        reason: reason.to_string(),
        cost_ms: 0.0,
        adapters: Vec::new(),
        rows: Vec::new(),
    }
}

/// 返回 false = 事件发送失败（通常是窗口已关闭）。
fn emit(app: &AppHandle, snapshot: GpuProcSnapshot) -> bool {
    app.emit_all("gpu-proc", snapshot).is_ok()
}

fn aggregate(sample: &Sample, names: &HashMap<u32, String>, elapsed: Duration) -> GpuProcSnapshot {
    aggregate_with_dxgi(sample, names, elapsed, &DXGI_ADAPTERS)
}

/// `aggregate` 的可注入版本：`dxgi` 为 LUID → DXGI 信息（生产路径传全局缓存，单测传构造映射）。
fn aggregate_with_dxgi(
    sample: &Sample,
    names: &HashMap<u32, String>,
    elapsed: Duration,
    dxgi: &HashMap<(u32, u32), DxgiAdapterInfo>,
) -> GpuProcSnapshot {
    // 每 PID：稳定引擎键 → (合并值, engtype 显示标签)
    let mut per_pid: HashMap<u32, HashMap<EngineKey, (f64, String)>> = HashMap::new();
    // 每适配器：eng → 跨 PID 求和
    let mut per_luid: HashMap<(u32, u32), HashMap<u32, f64>> = HashMap::new();
    // 每适配器：engtype → 跨 PID 求和（适配器的引擎分解，求和可 >100，不封顶）
    let mut per_luid_type: HashMap<(u32, u32), HashMap<String, f64>> = HashMap::new();

    for (inst, value) in &sample.items {
        let key = EngineKey {
            luid_hi: inst.luid_hi,
            luid_lo: inst.luid_lo,
            phys: inst.phys,
            eng: inst.eng,
        };
        let slot = per_pid
            .entry(inst.pid)
            .or_default()
            .entry(key)
            .or_insert_with(|| (0.0, inst.engtype.clone()));
        slot.0 += *value;

        *per_luid
            .entry((inst.luid_hi, inst.luid_lo))
            .or_default()
            .entry(inst.eng)
            .or_insert(0.0) += *value;

        // 空 engtype（Intel 核显实测存在）归为 (空)，便于显示
        let type_label = if inst.engtype.is_empty() {
            EMPTY_ENGTYPE_LABEL.to_string()
        } else {
            inst.engtype.clone()
        };
        *per_luid_type
            .entry((inst.luid_hi, inst.luid_lo))
            .or_default()
            .entry(type_label)
            .or_insert(0.0) += *value;
    }

    // 适配器：先按 (luid, eng) 跨 PID 求和，再取 max（与 Task Manager 性能页同一口径）
    let mut adapters: Vec<GpuAdapterRow> = per_luid
        .iter()
        .map(|(&(hi, lo), engines)| {
            let (kind, name) = classify_adapter_in(hi, lo, dxgi);
            // 显存：同一次 collect 读到的适配器 Usage；取不到该 LUID → 0
            let (dedicated, shared) = sample.mem.adapter.get(&(hi, lo)).copied().unwrap_or((0.0, 0.0));
            // 容量：DXGI DedicatedVideoMemory；virtual / 映射失败 → 0
            let capacity = dxgi
                .get(&(hi, lo))
                .map(|i| i.dedicated_video_memory as u64)
                .unwrap_or(0);
            // 引擎分解：按 engtype 求和、降序、取前 TOP_ADAPTER_ENGINES
            let mut engine_rows: Vec<GpuAdapterEngineRow> = per_luid_type
                .get(&(hi, lo))
                .map(|m| {
                    m.iter()
                        .map(|(engtype, pct)| GpuAdapterEngineRow {
                            engtype: engtype.clone(),
                            pct: *pct,
                        })
                        .collect()
                })
                .unwrap_or_default();
            engine_rows.sort_by(|a, b| {
                b.pct
                    .partial_cmp(&a.pct)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.engtype.cmp(&b.engtype))
            });
            engine_rows.truncate(TOP_ADAPTER_ENGINES);

            GpuAdapterRow {
                luid: luid_key(hi, lo),
                name,
                kind: kind.to_string(),
                gpu_pct: engines.values().copied().fold(0.0f64, f64::max),
                vram_dedicated_bytes: bytes_of(dedicated),
                vram_shared_bytes: bytes_of(shared),
                vram_capacity_bytes: capacity,
                engines: engine_rows,
            }
        })
        .collect();
    adapters.sort_by(|a, b| {
        b.gpu_pct
            .partial_cmp(&a.gpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.luid.cmp(&b.luid))
    });

    // 每 PID 的跨卡显存汇总（rows[] 既有的 vram_*_mb 是汇总口径）与"该 PID 有实例的卡"集合。
    // Process Memory 实例名自带 luid，所以逐卡拆分是精确的，不需要猜测归属。
    let mut proc_vram_total: HashMap<u32, (f64, f64)> = HashMap::new();
    let mut proc_luids: HashMap<u32, BTreeSet<(u32, u32)>> = HashMap::new();
    for ((pid, hi, lo), (ded, shr)) in &sample.mem.process {
        let total = proc_vram_total.entry(*pid).or_insert((0.0, 0.0));
        total.0 += *ded;
        total.1 += *shr;
        proc_luids.entry(*pid).or_default().insert((*hi, *lo));
    }

    let mut rows: Vec<GpuProcRow> = Vec::with_capacity(per_pid.len());
    for (&pid, engines) in &per_pid {
        // gpu_pct = 跨引擎取 max：先在 (luid, phys, eng) 上合并，再跨 luid 取最大
        let mut best: Option<(EngineKey, f64, &str)> = None;
        for (key, (sum, engtype)) in engines {
            if best.as_ref().map_or(true, |(_, b, _)| *sum > *b) {
                best = Some((*key, *sum, engtype.as_str()));
            }
        }
        // per_pid 中每个 PID 至少有一个引擎键，best 必为 Some；这里仍兜底避免 unwrap
        let Some((win_key, gpu_pct, win_engtype)) = best else {
            continue;
        };

        // gpu_sum_pct = 该 PID 在胜出适配器上的引擎求和（跨适配器求和是错的，会重复计数）
        let gpu_sum_pct: f64 = engines
            .iter()
            .filter(|(k, _)| k.luid_hi == win_key.luid_hi && k.luid_lo == win_key.luid_lo)
            .map(|(_, (sum, _))| *sum)
            .sum();
        if gpu_sum_pct <= 0.0 {
            continue;
        }

        let mut engine_rows: Vec<GpuEngineRow> = engines
            .iter()
            .filter(|(k, _)| k.luid_hi == win_key.luid_hi && k.luid_lo == win_key.luid_lo)
            .map(|(k, (sum, engtype))| GpuEngineRow {
                eng: k.eng,
                engtype: engtype.clone(),
                pct: *sum,
            })
            .collect();
        engine_rows.sort_by(|a, b| {
            b.pct
                .partial_cmp(&a.pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.eng.cmp(&b.eng))
        });
        engine_rows.truncate(TOP_ENGINES);

        // ---- per_adapter：该进程真正有实例的卡（Engine ∪ Process Memory）----
        let mut card_luids: BTreeSet<(u32, u32)> = BTreeSet::new();
        for k in engines.keys() {
            card_luids.insert((k.luid_hi, k.luid_lo));
        }
        if let Some(s) = proc_luids.get(&pid) {
            card_luids.extend(s.iter().copied());
        }
        let mut per_adapter: Vec<GpuProcAdapterRow> = card_luids
            .into_iter()
            .map(|(hi, lo)| {
                // 该卡上跨 eng 合并后的 max（engines 已按 (luid, phys, eng) 合并过）
                let card_pct = engines
                    .iter()
                    .filter(|(k, _)| k.luid_hi == hi && k.luid_lo == lo)
                    .map(|(_, (sum, _))| *sum)
                    .fold(0.0f64, f64::max);
                let (ded, shr) = sample
                    .mem
                    .process
                    .get(&(pid, hi, lo))
                    .copied()
                    .unwrap_or((0.0, 0.0));
                GpuProcAdapterRow {
                    luid: luid_key(hi, lo),
                    gpu_pct: card_pct,
                    vram_dedicated_mb: round1(ded.max(0.0) / MB),
                    vram_shared_mb: round1(shr.max(0.0) / MB),
                }
            })
            .collect();
        // 排序：gpu_pct 降序 → vram_dedicated_mb 降序 → luid 升序（渲染稳定）
        per_adapter.sort_by(|a, b| {
            b.gpu_pct
                .partial_cmp(&a.gpu_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.vram_dedicated_mb
                        .partial_cmp(&a.vram_dedicated_mb)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.luid.cmp(&b.luid))
        });
        per_adapter.truncate(TOP_PER_ADAPTER);

        rows.push(GpuProcRow {
            pid,
            name: names
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| format!("PID {}", pid)),
            gpu_pct,
            gpu_sum_pct,
            luid: luid_key(win_key.luid_hi, win_key.luid_lo),
            engtype: win_engtype.to_string(),
            // 每进程显存（**跨卡汇总**口径）：按 PID 把所有卡的实例相加，取不到记 0；
            // **不因此剔除该行**（过滤规则仍是 gpu_sum_pct > 0）
            vram_dedicated_mb: round1(
                proc_vram_total
                    .get(&pid)
                    .map(|m| m.0.max(0.0) / MB)
                    .unwrap_or(0.0),
            ),
            vram_shared_mb: round1(
                proc_vram_total
                    .get(&pid)
                    .map(|m| m.1.max(0.0) / MB)
                    .unwrap_or(0.0),
            ),
            engines: engine_rows,
            per_adapter,
        });
    }

    // 按 gpu_pct 降序；同值用 gpu_sum_pct 降序、PID 升序兜底，保证输出稳定可比对
    rows.sort_by(|a, b| {
        b.gpu_pct
            .partial_cmp(&a.gpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.gpu_sum_pct
                    .partial_cmp(&a.gpu_sum_pct)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.pid.cmp(&b.pid))
    });
    rows.truncate(TOP_N);

    GpuProcSnapshot {
        ts: now_hms(),
        available: true,
        reason: String::new(),
        cost_ms: round1(elapsed.as_secs_f64() * 1000.0),
        adapters,
        rows,
    }
}

// ---------------------------------------------------------------------------
// 线程主体
// ---------------------------------------------------------------------------

/// 启动后台 GPU 采集线程并立刻返回。重复调用是空操作（`STARTED` 守卫）。
pub fn start(app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || run(app));
}

fn run(app: AppHandle) {
    let mut warned_all_invalid = false;

    loop {
        // ---- 建立查询；计数器集缺失（虚拟机 / RDP / 老驱动）时退避重试，线程不退出 ----
        let mut query = match GpuQuery::open() {
            Ok(q) => q,
            Err(reason) => {
                reset_histogram();
                if !emit(&app, unavailable(&reason)) {
                    return;
                }
                std::thread::sleep(RETRY_BACKOFF);
                continue;
            }
        };

        // 预热采集并丢弃：速率计数器需要一对基准，否则首次读取
        // 返回 PDH_CSTATUS_INVALID_DATA (0xC0000BBA)（PoC 实测）。
        unsafe {
            PdhCollectQueryData(query.hquery);
        }

        let mut first = true;
        loop {
            std::thread::sleep(if first { FIRST_DELAY } else { CADENCE });
            first = false;

            // 进程名快照放在计时之外：走 2 秒 TTL 缓存，偶发一次全量枚举约 14 ms
            let names = crate::process::pid_name_map_cached();

            let t0 = Instant::now();
            let rc = unsafe { PdhCollectQueryData(query.hquery) };
            if rc != 0 {
                if is_counter_gone(rc) {
                    break;
                }
                // PDH_NO_DATA / PDH_INVALID_DATA 等瞬时失败：跳过本 tick，不发假 0，
                // 让前端保留上一份好数据
                continue;
            }

            let sample = match query.read() {
                Ok(s) => s,
                Err(rc) => {
                    if is_counter_gone(rc) {
                        break;
                    }
                    continue;
                }
            };

            if sample.items.is_empty() && sample.instances > 0 && !warned_all_invalid {
                warned_all_invalid = true;
                eprintln!(
                    "[gpu_proc] 一个 tick 内 {} 个实例全部被丢弃（单项 CStatus 无效 {}、实例名解析失败 {}），已跳过；后续同类情况不再重复打印",
                    sample.instances, sample.bad_items, sample.parse_fail
                );
            }

            let snapshot = aggregate(&sample, &names, t0.elapsed());
            // 发布槽先更新：即使 emit 失败（窗口关闭）历史侧也拿到的是本 tick 的数据
            publish_histogram(&snapshot);
            if !emit(&app, snapshot) {
                // 窗口已关闭：关闭查询并退出线程
                unsafe {
                    let _ = PdhCloseQuery(query.hquery);
                }
                return;
            }
        }

        // ---- 运行中计数器集消失（驱动重启 / GPU 复位）：关闭后走同一套退避重试 ----
        unsafe {
            let _ = PdhCloseQuery(query.hquery);
        }
        reset_histogram();
        if !emit(
            &app,
            unavailable("GPU 性能计数器集已消失（显卡驱动重启或 GPU 复位），稍后自动重试"),
        ) {
            return;
        }
        std::thread::sleep(RETRY_BACKOFF);
    }
}

// ---------------------------------------------------------------------------
// 单元测试：只覆盖纯逻辑（实例名解析 + 聚合口径），不触碰 PDH 与系统状态
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(pid: u32, lo: u32, eng: u32, engtype: &str) -> GpuInstance {
        GpuInstance {
            pid,
            luid_hi: 0,
            luid_lo: lo,
            phys: 0,
            eng,
            engtype: engtype.to_string(),
        }
    }

    #[test]
    fn parses_full_instance_name() {
        // 真实样例（PoC 证据文件）：engtype 含空格、hex 为大写
        let i = parse_instance(
            "pid_10668_luid_0x00000000_0x0000D969_phys_0_eng_10_engtype_High Priority 3D",
        )
        .expect("应能解析");
        assert_eq!(i.pid, 10668);
        assert_eq!(i.luid_hi, 0x0000_0000);
        assert_eq!(i.luid_lo, 0x0000_D969);
        assert_eq!(i.phys, 0);
        assert_eq!(i.eng, 10);
        assert_eq!(i.engtype, "High Priority 3D");
    }

    #[test]
    fn parses_empty_engtype() {
        // Intel 核显实测有 28 个 `..._engtype_` 空标签实例
        let i = parse_instance("pid_5364_luid_0x00000000_0x00010B34_phys_0_eng_11_engtype_")
            .expect("应能解析");
        assert_eq!(i.pid, 5364);
        assert_eq!(i.luid_lo, 0x0001_0B34);
        assert_eq!(i.eng, 11);
        assert_eq!(i.engtype, "");
    }

    #[test]
    fn parses_simplified_name_without_phys() {
        let i = parse_instance("pid_776_luid_0x00000000_0x0000D969_eng_2_engtype_VideoDecode")
            .expect("应能解析");
        assert_eq!(i.eng, 2);
        assert_eq!(i.phys, 0, "简化形式没有 _phys_，按 0 处理");
        assert_eq!(i.engtype, "VideoDecode");
    }

    #[test]
    fn malformed_names_are_rejected_without_panic() {
        for bad in [
            "",
            "garbage",
            "pid_x_luid_0x00000000_0x0000D969_phys_0_eng_0_engtype_3D",
            "pid_1_luid_0xZZ_0x0000D969_phys_0_eng_0_engtype_3D",
            "pid_1_phys_0_eng_0_engtype_3D",
        ] {
            assert!(parse_instance(bad).is_none(), "应拒绝: {bad}");
        }
    }

    #[test]
    fn aggregation_matches_frozen_contract() {
        let mut sample = Sample {
            items: vec![
                (inst(100, 0xD969, 0, "3D"), 5.0),
                (inst(100, 0xD969, 1, "Copy"), 1.0),
                (inst(100, 0x10B34, 0, "3D"), 2.0),
                (inst(200, 0xD969, 0, "3D"), 8.0),
                (inst(200, 0xD969, 0, "3D"), 0.5), // 同一稳定引擎键 → 先合并
                (inst(300, 0xD969, 3, "Compute 0"), 1.0),
                (inst(400, 0xD969, 0, "3D"), 0.0), // 零占用 → 不进 rows
            ],
            ..Default::default()
        };
        sample.instances = sample.items.len() as u32;

        let mut names = HashMap::new();
        names.insert(100u32, "a.exe".to_string());
        names.insert(200u32, "b.exe".to_string());

        let snap = aggregate(&sample, &names, Duration::from_micros(1900));
        assert!(snap.available);
        assert_eq!(snap.reason, "");
        assert_eq!(snap.cost_ms, 1.9);

        // rows：过滤零占用、按 gpu_pct 降序、名字缺失回退 "PID <n>"
        let pids: Vec<u32> = snap.rows.iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![200, 100, 300]);

        let p200 = &snap.rows[0];
        assert_eq!(p200.name, "b.exe");
        assert_eq!(p200.gpu_pct, 8.5, "同一 (luid, phys, eng) 上的实例先合并再取 max");
        assert_eq!(p200.gpu_sum_pct, 8.5);
        assert_eq!(p200.luid, "0x00000000:0x0000D969");

        let p100 = &snap.rows[1];
        assert_eq!(p100.gpu_pct, 5.0, "跨 luid 取最大，绝不跨适配器求和");
        assert_eq!(p100.gpu_sum_pct, 6.0, "只累加胜出适配器上的引擎");
        assert_eq!(p100.luid, "0x00000000:0x0000D969");
        assert_eq!(p100.engtype, "3D");
        assert_eq!(p100.engines.len(), 2);
        assert_eq!((p100.engines[0].eng, p100.engines[0].pct), (0, 5.0));
        assert_eq!(p100.engines[1].engtype, "Copy");

        assert_eq!(snap.rows[2].name, "PID 300");

        // adapters：先按 (luid, eng) 跨 PID 求和，再取 max（eng0 = 5 + 8.5 + 0）
        assert_eq!(snap.adapters.len(), 2);
        assert_eq!(snap.adapters[0].luid, "0x00000000:0x0000D969");
        assert_eq!(snap.adapters[0].gpu_pct, 13.5);
        assert_eq!(snap.adapters[1].luid, "0x00000000:0x00010B34");
        assert_eq!(snap.adapters[1].gpu_pct, 2.0);
    }

    #[test]
    fn rows_are_truncated_to_top_n() {
        let mut sample = Sample::default();
        for pid in 1..=(TOP_N as u32 + 10) {
            sample.items.push((inst(pid, 0xD969, 0, "3D"), pid as f64));
        }
        sample.instances = sample.items.len() as u32;

        let snap = aggregate(&sample, &HashMap::new(), Duration::from_millis(1));
        assert_eq!(snap.rows.len(), TOP_N);
        assert_eq!(snap.rows[0].pid, TOP_N as u32 + 10, "占用最高的排最前");
    }

    #[test]
    fn serialized_payload_matches_frozen_contract() {
        // 前端按冻结契约并行开发，这里逐字段核对真实序列化结果（键名 + 类型 + 取值形态）
        let mut sample = Sample::default();
        sample.items.push((inst(23728, 0xD969, 0, "3D"), 5.91));
        sample.instances = 1;
        let mut names = HashMap::new();
        names.insert(23728u32, "Trae.exe".to_string());

        let snap = aggregate(&sample, &names, Duration::from_micros(1900));
        let v = serde_json::to_value(&snap).expect("应能序列化");

        let keys = |val: &serde_json::Value| -> Vec<String> {
            let mut k: Vec<String> = val
                .as_object()
                .expect("应为 JSON 对象")
                .keys()
                .cloned()
                .collect();
            k.sort_unstable();
            k
        };

        assert_eq!(
            keys(&v),
            vec!["adapters", "available", "cost_ms", "reason", "rows", "ts"]
        );
        assert_eq!(v["available"].as_bool(), Some(true));
        assert_eq!(v["reason"].as_str(), Some(""));
        assert_eq!(v["cost_ms"].as_f64(), Some(1.9));
        let ts = v["ts"].as_str().expect("ts 应为字符串");
        assert_eq!(ts.len(), 8, "ts 形如 09:30:12（%H:%M:%S），实际 {ts}");
        assert_eq!(&ts[2..3], ":");

        let row = &v["rows"][0];
        assert_eq!(
            keys(row),
            vec![
                "engines",
                "engtype",
                "gpu_pct",
                "gpu_sum_pct",
                "luid",
                "name",
                "per_adapter",
                "pid",
                "vram_dedicated_mb",
                "vram_shared_mb"
            ]
        );
        assert_eq!(row["pid"].as_u64(), Some(23728));
        assert_eq!(row["name"].as_str(), Some("Trae.exe"));
        assert_eq!(row["gpu_pct"].as_f64(), Some(5.91));
        assert_eq!(row["gpu_sum_pct"].as_f64(), Some(5.91));
        assert_eq!(row["luid"].as_str(), Some("0x00000000:0x0000D969"));
        assert_eq!(row["engtype"].as_str(), Some("3D"));

        let engine = &row["engines"][0];
        assert_eq!(keys(engine), vec!["eng", "engtype", "pct"]);
        assert_eq!(engine["eng"].as_u64(), Some(0));
        assert_eq!(engine["engtype"].as_str(), Some("3D"));
        assert_eq!(engine["pct"].as_f64(), Some(5.91));

        let adapter = &v["adapters"][0];
        assert_eq!(
            keys(adapter),
            vec![
                "engines",
                "gpu_pct",
                "kind",
                "luid",
                "name",
                "vram_capacity_bytes",
                "vram_dedicated_bytes",
                "vram_shared_bytes"
            ]
        );
        assert_eq!(adapter["luid"].as_str(), Some("0x00000000:0x0000D969"));
        assert_eq!(adapter["gpu_pct"].as_f64(), Some(5.91));
        // name/kind 依赖本机 DXGI，故只断言与机器无关的不变量
        let kind = adapter["kind"].as_str().expect("kind 应为字符串");
        assert!(
            ["hardware", "software", "virtual"].contains(&kind),
            "kind 必须是三值之一，实际 {kind}"
        );
        let name = adapter["name"].as_str().expect("name 应为字符串");
        assert!(!name.is_empty(), "name 永不为空串");
    }

    #[test]
    fn unavailable_snapshot_has_same_shape() {
        // available:false 时字段一个不少（前端无需区分两种形状）
        let v = serde_json::to_value(unavailable("GPU 性能计数器不可用")).expect("应能序列化");
        assert_eq!(v["available"].as_bool(), Some(false));
        assert_eq!(v["reason"].as_str(), Some("GPU 性能计数器不可用"));
        assert_eq!(v["cost_ms"].as_f64(), Some(0.0));
        assert_eq!(v["rows"].as_array().map(|a| a.len()), Some(0));
        assert_eq!(v["adapters"].as_array().map(|a| a.len()), Some(0));
    }

    #[test]
    fn engine_rows_are_truncated_to_top_engines() {
        let mut sample = Sample::default();
        let n = TOP_ENGINES as u32 + 4;
        for eng in 0..n {
            sample.items.push((inst(7, 0xD969, eng, "Compute 0"), (eng + 1) as f64));
        }
        sample.instances = sample.items.len() as u32;

        let snap = aggregate(&sample, &HashMap::new(), Duration::from_millis(1));
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].engines.len(), TOP_ENGINES);
        assert_eq!(snap.rows[0].engines[0].eng, n - 1, "引擎明细按 pct 降序");
        assert_eq!(snap.rows[0].gpu_pct, n as f64);
        assert_eq!(snap.rows[0].gpu_sum_pct, (1..=n).sum::<u32>() as f64);
    }

    // ---------------- 显存（适配器 + 每进程）与适配器引擎分解 ----------------

    #[test]
    fn luid_instance_parsing_accepts_both_hex_cases() {
        // 大写来自 PDH/Rust 原始输出，小写来自 Get-Counter 本地化输出，两者都要接受
        assert_eq!(
            parse_luid_instance("luid_0x00000000_0x0000D969_phys_0"),
            Some((0, 0x0000_D969))
        );
        assert_eq!(
            parse_luid_instance("luid_0x00000000_0x0000d969_phys_0"),
            Some((0, 0x0000_D969))
        );
        assert_eq!(
            parse_luid_instance("luid_0x00000000_0x00010B34_phys_0"),
            Some((0, 0x0001_0B34))
        );
        // 没有 _phys_ 后缀也接受
        assert_eq!(
            parse_luid_instance("luid_0x00000000_0x0000D969"),
            Some((0, 0x0000_D969))
        );
        // 带 pid 段的每进程显存实例名不是 LUID 实例名
        assert_eq!(
            parse_luid_instance("pid_10668_luid_0x00000000_0x0000D969_phys_0"),
            None
        );
        for bad in ["", "luid_", "luid_0xZZ_0x0000D969_phys_0", "garbage"] {
            assert_eq!(parse_luid_instance(bad), None, "应拒绝: {bad}");
        }
    }

    #[test]
    fn adapter_vram_matches_by_luid_and_defaults_to_zero() {
        // 引擎侧两个 LUID，显存只有其中一个有数据 → 另一个记 0（不报错、不是 NaN）
        let mut dxgi = HashMap::new();
        dxgi.insert(
            (0u32, 0x0000_D969u32),
            dxgi_info_cap("AMD Radeon RX 6750 XT", 0, 12_836_904_960),
        );
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 5.0));
        sample.items.push((inst(200, 0x10B34, 0, "3D"), 1.0));
        sample.instances = 2;
        sample
            .mem
            .adapter
            .insert((0, 0xD969), (5_362_176_000.0, 2_517_450_752.0));

        let snap = aggregate_with_dxgi(&sample, &HashMap::new(), Duration::from_millis(1), &dxgi);
        let a0 = snap
            .adapters
            .iter()
            .find(|a| a.luid.ends_with("D969"))
            .expect("应有 0xD969");
        assert_eq!(a0.vram_dedicated_bytes, 5_362_176_000);
        assert_eq!(a0.vram_shared_bytes, 2_517_450_752);
        assert_eq!(a0.vram_capacity_bytes, 12_836_904_960, "容量来自 DXGI");
        assert_eq!(a0.kind, "hardware");

        let a1 = snap
            .adapters
            .iter()
            .find(|a| a.luid.ends_with("0B34"))
            .expect("应有 0x10B34");
        assert_eq!(
            (
                a1.vram_dedicated_bytes,
                a1.vram_shared_bytes,
                a1.vram_capacity_bytes
            ),
            (0, 0, 0),
            "未匹配的 LUID 三项都记 0"
        );
        assert_eq!(a1.kind, "virtual", "DXGI 映射里没有该 LUID");
        assert_eq!(a1.name, "Adapter 00010B34");

        // 异常值也归 0
        assert_eq!(bytes_of(-1.0), 0);
        assert_eq!(bytes_of(f64::NAN), 0);
        assert_eq!(bytes_of(0.0), 0);
        assert_eq!(bytes_of(1234.9), 1234);
    }

    #[test]
    fn adapter_engine_breakdown_sums_by_engtype_desc_and_truncates() {
        let mut sample = Sample::default();
        // 同一 LUID 上 6 类引擎：3D 跨两个 PID 求和 = 4.5，其余各 1 条
        sample.items.push((inst(1, 0xD969, 0, "3D"), 3.0));
        sample.items.push((inst(2, 0xD969, 0, "3D"), 1.5));
        sample.items.push((inst(1, 0xD969, 1, "Copy"), 2.0));
        sample.items.push((inst(1, 0xD969, 2, ""), 1.0)); // 空 engtype → (空)
        sample.items.push((inst(1, 0xD969, 3, "Compute 0"), 0.5));
        sample.items.push((inst(1, 0xD969, 4, "Timer 0"), 0.25));
        sample.items.push((inst(1, 0xD969, 5, "VideoDecode"), 0.1));
        sample.instances = sample.items.len() as u32;

        let empty_dxgi: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty_dxgi,
        );
        assert_eq!(snap.adapters.len(), 1);
        let got: Vec<(String, f64)> = snap.adapters[0]
            .engines
            .iter()
            .map(|e| (e.engtype.clone(), e.pct))
            .collect();
        assert_eq!(
            got,
            vec![
                ("3D".to_string(), 4.5),
                ("Copy".to_string(), 2.0),
                ("(空)".to_string(), 1.0),
                ("Compute 0".to_string(), 0.5),
            ],
            "按 (luid, engtype) 跨 PID 求和、降序、截断前 {TOP_ADAPTER_ENGINES}，空串归一为 (空)"
        );

        // 求和可 >100：不封顶
        sample.items.push((inst(3, 0xD969, 0, "3D"), 60.0));
        sample.items.push((inst(4, 0xD969, 0, "3D"), 50.0));
        let snap2 = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty_dxgi,
        );
        let three_d = snap2.adapters[0]
            .engines
            .iter()
            .find(|e| e.engtype == "3D")
            .expect("应有 3D");
        assert_eq!(three_d.pct, 114.5, "跨 PID 求和可以 >100，不封顶");
    }

    #[test]
    fn process_vram_missing_is_zero_and_row_is_kept() {
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 5.0));
        sample.items.push((inst(200, 0xD969, 0, "3D"), 3.0));
        sample.instances = 2;
        // 只给 PID 100：412.5 MB / 88.3 MB；PID 200 缺失
        sample
            .mem
            .process
            .insert((100, 0, 0xD969), (432_537_600.0, 92_602_368.0));

        let empty_dxgi: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty_dxgi,
        );
        assert_eq!(snap.rows.len(), 2, "显存缺失不影响行是否保留");
        let r100 = snap.rows.iter().find(|r| r.pid == 100).expect("应有 PID 100");
        assert_eq!(r100.vram_dedicated_mb, 412.5);
        assert_eq!(r100.vram_shared_mb, 88.3);
        let r200 = snap.rows.iter().find(|r| r.pid == 200).expect("应有 PID 200");
        assert_eq!(
            (r200.vram_dedicated_mb, r200.vram_shared_mb),
            (0.0, 0.0),
            "取不到的 PID 记 0，且该行仍然保留"
        );
    }

    // ---------------- rows[].per_adapter 逐卡分解 ----------------

    #[test]
    fn per_adapter_single_card_matches_summary() {
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 1.25));
        sample.instances = 1;
        sample
            .mem
            .process
            .insert((100, 0, 0xD969), (1_234_567_890.0, 78_643_200.0));

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.per_adapter.len(), 1, "单卡进程只有一条分解");
        let p = &r.per_adapter[0];
        assert_eq!(p.luid, "0x00000000:0x0000D969");
        assert_eq!(p.gpu_pct, 1.25);
        assert_eq!(p.gpu_pct, r.gpu_pct, "单卡时分解值 == 汇总值");
        assert_eq!(p.vram_dedicated_mb, r.vram_dedicated_mb);
        assert_eq!(p.vram_shared_mb, r.vram_shared_mb);
        assert_eq!(p.vram_shared_mb, 75.0);
    }

    #[test]
    fn per_adapter_splits_by_pid_and_luid_in_rule_order() {
        let mut sample = Sample::default();
        // 同一 PID 在两张卡上都有引擎实例：卡 B 更忙
        sample.items.push((inst(100, 0xD969, 0, "3D"), 2.0));
        sample.items.push((inst(100, 0x10B34, 0, "3D"), 5.0));
        sample.instances = 2;
        // (pid, luid) 精确匹配的显存：卡 B 专用更大，但占用更低
        sample
            .mem
            .process
            .insert((100, 0, 0xD969), (100_000_000.0, 0.0));
        sample
            .mem
            .process
            .insert((100, 0, 0x10B34), (200_000_000.0, 0.0));

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.per_adapter.len(), 2);
        // 规则 4：gpu_pct 降序 → 卡 B 在前
        assert_eq!(r.per_adapter[0].luid, "0x00000000:0x00010B34");
        assert_eq!(r.per_adapter[0].gpu_pct, 5.0);
        assert_eq!(r.per_adapter[1].luid, "0x00000000:0x0000D969");
        assert_eq!(r.per_adapter[1].gpu_pct, 2.0);
        // 汇总 = 跨卡 max，不是求和
        assert_eq!(r.gpu_pct, 5.0);
        assert_eq!(r.luid, "0x00000000:0x00010B34");
        // 显存按 (pid, luid) 精确匹配，两张卡不互相污染
        assert_eq!(r.per_adapter[0].vram_dedicated_mb, 190.7);
        assert_eq!(r.per_adapter[1].vram_dedicated_mb, 95.4);
        assert_eq!(r.vram_dedicated_mb, 286.1, "汇总字段是跨卡求和");
    }

    #[test]
    fn per_adapter_keeps_card_with_engine_only_vram_zero() {
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 3.0));
        sample.items.push((inst(100, 0x10B34, 0, "3D"), 0.0)); // 有实例但 0 占用
        sample.instances = 2;
        // 只有卡 A 有 Process Memory 实例
        sample
            .mem
            .process
            .insert((100, 0, 0xD969), (50_000_000.0, 0.0));

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.per_adapter.len(), 2, "只有 Engine 实例的卡也要出现");
        let b = r
            .per_adapter
            .iter()
            .find(|p| p.luid.ends_with("0B34"))
            .expect("应有 0x10B34");
        assert_eq!(b.gpu_pct, 0.0);
        assert_eq!(
            (b.vram_dedicated_mb, b.vram_shared_mb),
            (0.0, 0.0),
            "该卡没有 Process Memory 实例 → 显存 0"
        );
    }

    #[test]
    fn per_adapter_keeps_card_with_memory_only_gpu_zero() {
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 1.0)); // 只有卡 A 有引擎实例
        sample.instances = 1;
        sample
            .mem
            .process
            .insert((100, 0, 0xD969), (10_000_000.0, 0.0));
        sample
            .mem
            .process
            .insert((100, 0, 0x10B34), (20_000_000.0, 5_000_000.0)); // 卡 B 只有显存实例

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.per_adapter.len(), 2, "只有 Process Memory 实例的卡也要出现");
        let b = r
            .per_adapter
            .iter()
            .find(|p| p.luid.ends_with("0B34"))
            .expect("应有 0x10B34");
        assert_eq!(b.gpu_pct, 0.0, "该卡没有引擎实例 → gpu_pct 0");
        assert_eq!(b.vram_dedicated_mb, 19.1);
        assert_eq!(b.vram_shared_mb, 4.8);
    }

    #[test]
    fn per_adapter_truncates_to_top_four() {
        let mut sample = Sample::default();
        for (i, lo) in [0x1000u32, 0x2000, 0x3000, 0x4000, 0x5000].iter().enumerate() {
            sample.items.push((inst(100, *lo, 0, "3D"), (i + 1) as f64));
        }
        sample.instances = 5;

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.per_adapter.len(), TOP_PER_ADAPTER);
        let pcts: Vec<f64> = r.per_adapter.iter().map(|p| p.gpu_pct).collect();
        assert_eq!(pcts, vec![5.0, 4.0, 3.0, 2.0], "按 gpu_pct 降序截断到 4");
    }

    #[test]
    fn summary_and_per_adapter_use_different_aggregation() {
        let mut sample = Sample::default();
        sample.items.push((inst(100, 0xD969, 0, "3D"), 1.0));
        sample.items.push((inst(100, 0x10B34, 0, "3D"), 2.0));
        sample.instances = 2;

        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        let snap = aggregate_with_dxgi(
            &sample,
            &HashMap::new(),
            Duration::from_millis(1),
            &empty,
        );
        let r = &snap.rows[0];
        assert_eq!(r.gpu_pct, 2.0, "汇总 = 跨卡 max（不是 1.0+2.0=3.0）");
        assert_eq!(r.gpu_sum_pct, 2.0, "gpu_sum_pct 只累加胜出卡");
        assert_eq!(r.per_adapter.len(), 2, "分解保留两张卡");
        assert_eq!(
            r.per_adapter.iter().map(|p| p.gpu_pct).sum::<f64>(),
            3.0,
            "分解两条相加才是 3.0"
        );
    }

    // ---------------- DXGI 适配器命名 / kind 判定 ----------------

    fn dxgi_info(name: &str, flags: u32) -> DxgiAdapterInfo {
        DxgiAdapterInfo {
            name: name.to_string(),
            flags,
            dedicated_video_memory: 0,
        }
    }

    fn dxgi_info_cap(name: &str, flags: u32, capacity: usize) -> DxgiAdapterInfo {
        DxgiAdapterInfo {
            name: name.to_string(),
            flags,
            dedicated_video_memory: capacity,
        }
    }

    #[test]
    fn adapter_kind_three_branches() {
        // 注入的 DXGI 映射（不依赖本机硬件）：一个硬件适配器 + 一个软件适配器
        let mut dxgi = HashMap::new();
        dxgi.insert((0u32, 0x0000_D969u32), dxgi_info("AMD Radeon RX 6750 XT", 0));
        dxgi.insert(
            (0u32, 0x0001_0ADCu32),
            dxgi_info("Microsoft Basic Render Driver", ADAPTER_FLAG_SOFTWARE_U32),
        );

        // 分支 1：匹配到 + Flags & 0x2 == 0 → hardware
        assert_eq!(
            classify_adapter_in(0, 0x0000_D969, &dxgi),
            ("hardware", "AMD Radeon RX 6750 XT".to_string())
        );
        // 分支 2：匹配到 + Flags & 0x2 != 0 → software
        assert_eq!(
            classify_adapter_in(0, 0x0001_0ADC, &dxgi),
            ("software", "Microsoft Basic Render Driver".to_string())
        );
        // 分支 3：DXGI 里没有该 LUID → virtual（真实样例：本机 DXGI 有 4 个适配器而计数器只有 3 个 LUID）
        assert_eq!(
            classify_adapter_in(0, 0x0001_BA6C, &dxgi),
            ("virtual", "Adapter 0001BA6C".to_string())
        );
        // 必须按 (HighPart, LowPart) 原序匹配：高位不同不算匹配
        assert_eq!(classify_adapter_in(1, 0x0000_D969, &dxgi).0, "virtual");
    }

    #[test]
    fn adapter_name_fallback_when_dxgi_empty_or_description_blank() {
        // DXGI 整体失败 / 枚举为空 → 全部 virtual，名字兜底且非空
        let empty: HashMap<(u32, u32), DxgiAdapterInfo> = HashMap::new();
        for lo in [0x0000_D969u32, 0x0001_0B34, 0x0001_0ADC] {
            let (kind, name) = classify_adapter_in(0, lo, &empty);
            assert_eq!(kind, "virtual");
            assert_eq!(name, format!("Adapter {lo:08X}"));
            assert!(!name.is_empty(), "name 永不为空串");
        }

        // 匹配到但 Description 为空 → kind 仍按 flags 判定，名字走兜底
        let mut dxgi = HashMap::new();
        dxgi.insert((0u32, 0x0001_0B34u32), dxgi_info("", 0));
        assert_eq!(
            classify_adapter_in(0, 0x0001_0B34, &dxgi),
            ("hardware", "Adapter 00010B34".to_string())
        );
        dxgi.insert((0u32, 0x0001_0B35u32), dxgi_info("", ADAPTER_FLAG_SOFTWARE_U32));
        assert_eq!(
            classify_adapter_in(0, 0x0001_0B35, &dxgi),
            ("software", "Adapter 00010B35".to_string())
        );
    }

    #[test]
    fn dxgi_description_is_trimmed_at_nul_and_whitespace() {
        let mut desc = [0u16; 128];
        let text: Vec<u16> = "  Intel(R) UHD Graphics 770  \0".encode_utf16().collect();
        desc[..text.len()].copy_from_slice(&text);
        assert_eq!(trim_utf16_desc(&desc), "Intel(R) UHD Graphics 770");
        // 无 NUL 结尾（写满 128）也不越界
        let full: Vec<u16> = "A".repeat(128).encode_utf16().collect();
        let mut desc_full = [0u16; 128];
        desc_full.copy_from_slice(&full);
        assert_eq!(trim_utf16_desc(&desc_full), "A".repeat(128));
        // 全 0 → 空串，由调用方兜底
        assert_eq!(trim_utf16_desc(&[0u16; 128]), "");
    }

    #[test]
    fn histogram_slot_is_default_until_first_tick_then_truncates_to_eight() {
        // 未就绪（尚未采到第一个 tick / 计数器不可用）→ Default
        reset_histogram();
        let d = histogram_sample();
        assert!(!d.available);
        assert_eq!(d.gpu_pct, 0.0);
        assert!(d.luid.is_empty());
        assert!(d.adapters.is_empty());

        // available:false 的快照发布后仍是 Default 形态
        publish_histogram(&unavailable("GPU 性能计数器不可用"));
        assert!(!histogram_sample().available);

        // 12 个适配器 → 截断到 HIST_ADAPTERS(8)，胜出者与 payload 的 adapters[0] 一致
        let adapters: Vec<GpuAdapterRow> = (0..12u32)
            .map(|i| GpuAdapterRow {
                luid: luid_key(0, 0xD000 + i),
                name: format!("Adapter {i}"),
                kind: "hardware".to_string(),
                gpu_pct: (12 - i) as f64,
                vram_dedicated_bytes: 0,
                vram_shared_bytes: 0,
                vram_capacity_bytes: 0,
                engines: Vec::new(),
            })
            .collect();
        let snap = GpuProcSnapshot {
            ts: "20:00:00".to_string(),
            available: true,
            reason: String::new(),
            cost_ms: 1.0,
            adapters,
            rows: Vec::new(),
        };
        publish_histogram(&snap);

        let h = histogram_sample();
        assert!(h.available);
        assert_eq!(h.adapters.len(), HIST_ADAPTERS);
        assert_eq!(h.gpu_pct, snap.adapters[0].gpu_pct, "胜出者 = payload adapters[0]");
        assert_eq!(h.luid, snap.adapters[0].luid);
        assert_eq!(h.adapters[0].name, snap.adapters[0].name);
        assert_eq!(h.adapters[0].luid, snap.adapters[0].luid);
        let pcts: Vec<f64> = h.adapters.iter().map(|a| a.gpu_pct).collect();
        assert_eq!(pcts, vec![12.0, 11.0, 10.0, 9.0, 8.0, 7.0, 6.0, 5.0]);

        // 计数器消失 → 复位
        reset_histogram();
        assert!(!histogram_sample().available);
    }

    /// 真机证据：打印 DXGI 实际枚举结果 + 引擎侧实际出现的 LUID 及各自 kind。
    /// 手动运行：`cargo test --release live_dxgi_adapters_and_kinds -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本机真实 DXGI / GPU 计数器；用 -- --ignored 手动运行"]
    fn live_dxgi_adapters_and_kinds() {
        let (list, err) = enumerate_dxgi_adapters();
        println!("[dxgi] 枚举失败/中断原因: {err:?}");
        println!("[dxgi] DXGI 适配器共 {} 个", list.len());
        for (hi, lo, info) in &list {
            let (kind, name) = classify_adapter_in(*hi, *lo, &DXGI_ADAPTERS);
            println!(
                "[dxgi]   0x{hi:08X}:0x{lo:08X}  flags=0x{:X}  kind={kind:<8} name={name:?}",
                info.flags
            );
            assert!(!name.is_empty(), "name 永不为空串");
            assert!(["hardware", "software", "virtual"].contains(&kind));
        }
        assert!(!list.is_empty(), "本机应有 DXGI 适配器");

        // 引擎侧实际出现的 LUID（真实 PDH 路径）
        let mut query = GpuQuery::open().expect("本机应能打开 GPU Engine 计数器");
        unsafe {
            PdhCollectQueryData(query.hquery);
        }
        std::thread::sleep(Duration::from_millis(1100));
        unsafe {
            PdhCollectQueryData(query.hquery);
        }
        let sample = query.read().expect("读取实例数组失败");
        let mut luids: Vec<(u32, u32)> = sample
            .items
            .iter()
            .map(|(i, _)| (i.luid_hi, i.luid_lo))
            .collect();
        luids.sort_unstable();
        luids.dedup();
        println!(
            "[dxgi] 引擎实例 {} 个（有效 {}），出现 {} 个 LUID:",
            sample.instances,
            sample.items.len(),
            luids.len()
        );
        for (hi, lo) in &luids {
            let (kind, name) = classify_adapter_in(*hi, *lo, &DXGI_ADAPTERS);
            let in_dxgi = list.iter().any(|(h, l, _)| h == hi && l == lo);
            println!(
                "[dxgi]   0x{hi:08X}:0x{lo:08X}  in_dxgi={in_dxgi}  kind={kind:<8} name={name:?}"
            );
        }
        let dxgi_only: Vec<String> = list
            .iter()
            .filter(|(h, l, _)| !luids.contains(&(*h, *l)))
            .map(|(h, l, _)| format!("0x{h:08X}:0x{l:08X}"))
            .collect();
        println!("[dxgi] DXGI 有而引擎侧没有: {dxgi_only:?}");
        println!("[dxgi] 引擎侧有而 DXGI 没有: {:?}", {
            let v: Vec<String> = luids
                .iter()
                .filter(|(h, l)| !list.iter().any(|(hh, ll, _)| hh == h && ll == l))
                .map(|(h, l)| format!("0x{h:08X}:0x{l:08X}"))
                .collect();
            v
        });
    }

    /// 系统级联调测试：跑**真实的** PDH 路径（open → 预热 → 连续多 tick 读取 → 解析 →
    /// 聚合 → 序列化），验证在真机上确实能取到每进程 GPU 数据，而不只是合成数据能过。
    ///
    /// 依赖本机存在 WDDM 2.0+ 显卡，故默认 `#[ignore]`，不进 CI。
    /// 手动运行：
    ///   cargo test --release live_gpu_query_produces_real_rows -- --ignored --nocapture
    #[test]
    #[ignore = "需要本机真实 GPU 计数器；用 -- --ignored 手动运行"]
    fn live_gpu_query_produces_real_rows() {
        let mut query = GpuQuery::open().expect("本机应能打开 GPU Engine 计数器");

        // 预热采集并丢弃：速率计数器需要一对基准
        unsafe {
            PdhCollectQueryData(query.hquery);
        }

        let names = crate::process::pid_name_map_cached();
        println!("[live] 进程名快照 {} 条", names.len());

        let mut last: Option<GpuProcSnapshot> = None;
        let mut total_instances = 0usize;
        let mut ticks_with_rows = 0usize;

        for tick in 0..3 {
            std::thread::sleep(Duration::from_millis(1100));

            let t0 = Instant::now();
            let rc = unsafe { PdhCollectQueryData(query.hquery) };
            assert_eq!(rc, 0, "第 {tick} 个 tick 的 PdhCollectQueryData 失败：0x{rc:08X}");

            let sample = query.read().expect("读取实例数组失败");
            total_instances += sample.instances as usize;
            println!(
                "[live] tick {tick}: 实例 {}，有效 {}，单项无效 {}，解析失败 {}",
                sample.instances,
                sample.items.len(),
                sample.bad_items,
                sample.parse_fail
            );

            let snap = aggregate(&sample, &names, t0.elapsed());
            if !snap.rows.is_empty() {
                ticks_with_rows += 1;
            }
            println!(
                "      耗时 {:.2} ms，rows {}，adapters {}",
                snap.cost_ms,
                snap.rows.len(),
                snap.adapters.len()
            );
            for r in snap.rows.iter().take(8) {
                println!(
                    "      {:<24} PID {:<7} max {:>6.2}%  sum {:>6.2}%  VRAM {:>9.1} / {:>9.1} MB  引擎 {}",
                    r.name, r.pid, r.gpu_pct, r.gpu_sum_pct, r.vram_dedicated_mb, r.vram_shared_mb, r.engtype
                );
                for p in &r.per_adapter {
                    println!(
                        "          per_adapter {} gpu {:>7.3}%  vram {:>9.1} / {:>9.1} MB",
                        p.luid, p.gpu_pct, p.vram_dedicated_mb, p.vram_shared_mb
                    );
                }
            }
            for a in &snap.adapters {
                println!(
                    "      [适配器] {} {:<30} kind={:<8} gpu {:>6.2}%",
                    a.luid, a.name, a.kind, a.gpu_pct
                );
                println!(
                    "               Dedicated {:>14} B ({:>7.3} GiB)  Shared {:>14} B ({:>7.3} GiB)  Capacity {:>14} B ({:>7.3} GiB)",
                    a.vram_dedicated_bytes,
                    a.vram_dedicated_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
                    a.vram_shared_bytes,
                    a.vram_shared_bytes as f64 / 1024.0 / 1024.0 / 1024.0,
                    a.vram_capacity_bytes,
                    a.vram_capacity_bytes as f64 / 1024.0 / 1024.0 / 1024.0
                );
                for e in &a.engines {
                    println!("               engtype {:<24} {:>8.2}%", e.engtype, e.pct);
                }
            }
            last = Some(snap);
        }

        let snap = last.expect("至少跑过一个 tick");

        // 真实数据的硬性不变量
        assert!(total_instances > 0, "连续 3 个 tick 都没读到任何 GPU Engine 实例");
        assert!(
            ticks_with_rows > 0,
            "连续 3 个 tick 都没有任何非零 GPU 占用（本机应有 GPU 活动）"
        );
        for r in &snap.rows {
            assert!(
                (0.0..=100.0).contains(&r.gpu_pct),
                "gpu_pct 必须落在 0..=100（最忙引擎口径），实际 {}（{}）",
                r.gpu_pct,
                r.name
            );
            assert!(
                r.gpu_sum_pct >= r.gpu_pct - 1e-9,
                "引擎求和应不小于最大值：{} < {}",
                r.gpu_sum_pct,
                r.gpu_pct
            );
            assert!(
                r.luid.starts_with("0x") && r.luid.contains(':'),
                "luid 应形如 0x…:0x…，实际 {}",
                r.luid
            );
            for e in &r.engines {
                assert!(
                    e.pct <= r.gpu_sum_pct + 1e-9,
                    "单引擎 {} 不应超过求和 {}",
                    e.pct,
                    r.gpu_sum_pct
                );
            }
        }
        assert!(snap.adapters.iter().all(|a| (0.0..=100.0).contains(&a.gpu_pct)));

        // 新增字段的不变量：引擎分解截断/降序、显存非负、本机至少一个适配器有 DXGI 容量
        for a in &snap.adapters {
            assert!(
                a.engines.len() <= TOP_ADAPTER_ENGINES,
                "{} 的引擎分解超过 {TOP_ADAPTER_ENGINES} 条",
                a.luid
            );
            for w in a.engines.windows(2) {
                assert!(w[0].pct >= w[1].pct, "引擎分解必须按 pct 降序：{:?}", a.engines);
            }
        }
        assert!(
            snap.adapters.iter().any(|a| a.vram_capacity_bytes > 0),
            "本机应有至少一个 DXGI 报出 DedicatedVideoMemory 的适配器"
        );
        for r in &snap.rows {
            assert!(r.vram_dedicated_mb >= 0.0 && r.vram_shared_mb >= 0.0);
            assert!(
                r.per_adapter.len() <= TOP_PER_ADAPTER,
                "{} 的逐卡分解超过 {TOP_PER_ADAPTER} 条",
                r.name
            );
            for w in r.per_adapter.windows(2) {
                assert!(
                    w[0].gpu_pct >= w[1].gpu_pct,
                    "逐卡分解必须按 gpu_pct 降序：{:?}",
                    r.per_adapter
                );
            }
            // 分解里的每张卡都必须真的在 adapters[] 里（前端用 adapters 做 luid→name 映射）
            for p in &r.per_adapter {
                assert!(
                    snap.adapters.iter().any(|a| a.luid == p.luid),
                    "per_adapter 的 luid {} 必须出现在 adapters[] 里",
                    p.luid
                );
            }
        }

        // 跨卡进程的实际内容 + "非最忙卡是否有非零占用"（Lead 关注的判据）
        let multi: Vec<&GpuProcRow> = snap
            .rows
            .iter()
            .filter(|r| r.per_adapter.len() >= 2)
            .collect();
        println!("[live] 跨卡进程（per_adapter ≥ 2 条）: {} 个", multi.len());
        for r in multi.iter().take(6) {
            println!(
                "      {} PID {} 汇总 gpu {:.3}% 胜出卡 {} 汇总显存 {:.1}/{:.1} MB",
                r.name, r.pid, r.gpu_pct, r.luid, r.vram_dedicated_mb, r.vram_shared_mb
            );
            for p in &r.per_adapter {
                println!(
                    "          {} gpu {:>7.3}%  vram {:>9.1} / {:>9.1} MB",
                    p.luid, p.gpu_pct, p.vram_dedicated_mb, p.vram_shared_mb
                );
            }
        }
        let nonbusy_nonzero: Vec<String> = snap
            .rows
            .iter()
            .flat_map(|r| {
                r.per_adapter
                    .iter()
                    .skip(1)
                    .filter(|p| p.gpu_pct > 0.0)
                    .map(move |p| format!("{} (PID {}) {} = {:.3}%", r.name, r.pid, p.luid, p.gpu_pct))
            })
            .collect();
        println!(
            "[live] 非最忙卡上的非零占用（预期为空）: {} 条 {:?}",
            nonbusy_nonzero.len(),
            nonbusy_nonzero
        );
        println!(
            "[live] 显存/引擎分解汇总：{}",
            snap.adapters
                .iter()
                .map(|a| format!(
                    "{} {} ded={}B shared={}B cap={}B engines={}",
                    a.luid,
                    a.kind,
                    a.vram_dedicated_bytes,
                    a.vram_shared_bytes,
                    a.vram_capacity_bytes,
                    a.engines.len()
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        );

        // 序列化后必须仍是冻结契约的形状
        let v = serde_json::to_value(&snap).expect("快照应能序列化");
        for k in ["ts", "available", "reason", "cost_ms", "adapters", "rows"] {
            assert!(v.get(k).is_some(), "缺少契约字段 {k}");
        }
        println!(
            "[live] 通过：rows={} adapters={} available={}",
            snap.rows.len(),
            snap.adapters.len(),
            snap.available
        );

        // 供前端渲染验证使用的真实 payload（原样来自本机实采）
        println!(
            "[live-json] {}",
            serde_json::to_string(&snap).expect("快照应能序列化")
        );
    }
}
