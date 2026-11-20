//! 自动加速：内存占用持续高于阈值时自动裁剪各进程工作集（仅释放内存，不删任何文件）。
//!
//! 触发以「加速效果」为准，而不是固定的冷却时间：
//! * 持续性：内存连续 45 秒 ≥ 阈值才触发，过滤一闪而过的尖峰；
//! * 观察：加速后观察 30 秒，看内存最终稳定在哪里——
//!   - 回落到阈值以下（留 3 个百分点余量）：加速有效，下次超阈值可立即再次加速，没有冷却等待；
//!   - 仍高于阈值：说明是程序的真实占用，加速解决不了，暂停重复加速，直到内存回落到阈值以下，
//!     或比加速后的低点再上涨 5 个百分点（又积累了可释放的内存）；低点会跟随内存下降而更新；
//! * 紧急：内存 ≥ 95% 持续 10 秒立即处理，不受全屏 / 磁盘繁忙守卫限制（此时的磁盘繁忙
//!   多半就是内存不足引起的换页）；只有加速后本身就稳定在 95% 附近时才不再重复；
//! * 守卫：常规触发在电池供电、游戏 / 演示全屏、磁盘繁忙时暂停；紧急触发只尊重电池供电。
//!   普通最大化窗口不算全屏（任务栏自动隐藏时最大化窗口同样覆盖整个屏幕）。

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::memboost;

pub const DEFAULT_THRESHOLD: u8 = 85;
pub const THRESHOLD_MIN: u8 = 50;
pub const THRESHOLD_MAX: u8 = 95;
/// 一次加速至少释放这么多内存才弹提示（避免「释放 0 MB」之类的噪音）。
const MEANINGFUL_FREE_MB: f64 = 50.0;
/// 常规触发：持续超过触发线多久（45 秒）。
const SUSTAINED_SECS: u64 = 45;
/// 加速后的观察期：用这段时间之后的内存水平判断加速是否有效。
const SETTLE_SECS: u64 = 30;
/// 判定「已回落到阈值以下」时保留的余量（百分点），避免在阈值附近来回抖动。
const REARM_MARGIN: u32 = 3;
/// 暂停期内，内存比加速后的低点再上涨这么多（百分点）才再次加速。
const RISE_STEP: u32 = 5;
/// 磁盘 % Disk Time 超过该值视为繁忙，暂停常规触发。
const DISK_BUSY_PERCENT: f64 = 80.0;
/// 紧急阈值：内存达到该值说明系统即将开始大量换页、出现卡死。
const EMERGENCY_PERCENT: u32 = 95;
/// 紧急触发：持续多久（10 秒）。
const EMERGENCY_SUSTAINED_SECS: u64 = 10;

/// 检测周期。5 秒一次读取内存占用（GlobalMemoryStatusEx，开销可忽略）。
const TICK: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_enabled")]
    pub auto_boost_enabled: bool,
    #[serde(default = "default_threshold")]
    pub auto_boost_threshold: u8,
}

fn default_enabled() -> bool {
    true
}

fn default_threshold() -> u8 {
    DEFAULT_THRESHOLD
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_boost_enabled: true,
            auto_boost_threshold: DEFAULT_THRESHOLD,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct BoostEvent {
    pub ts_ms: i64,
    pub mem_load: u32,
    pub freed_mb: f64,
    pub trimmed_count: u32,
}

/// 前端展示用的状态（`get_auto_boost` 返回值，也随 `auto-boost-state` 事件推送）。
#[derive(Serialize, Clone)]
pub struct AutoBoostState {
    pub enabled: bool,
    pub threshold: u8,
    /// armed：可触发；settling：刚加速完、观察效果中；paused：加速后仍高于阈值，暂停重复加速
    pub status: &'static str,
    /// 暂停时：加速后的内存低点（%）
    pub paused_floor: Option<u32>,
    /// 暂停时：内存升到该值及以上会再次加速（%）
    pub resume_above: Option<u32>,
    /// 内存回落到该值及以下即恢复正常触发（%）
    pub rearm_at: u32,
    pub last: Option<BoostEvent>,
}

/// 触发通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// 常规：持续超过触发线 45 秒，受全部场景守卫约束。
    Normal,
    /// 紧急：内存 ≥ 95% 持续 10 秒，只尊重电池供电。
    Emergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 可触发：内存持续超过阈值即加速。
    Armed,
    /// 刚加速完，观察到 `until_ms` 再判断效果；`boosted_at` 为加速时的内存占用。
    Settling { until_ms: i64, boosted_at: u32 },
    /// 加速后仍高于阈值（真实占用）：暂停重复加速；`floor` 为加速后的内存低点。
    Paused { floor: u32 },
}

/// 触发状态机。只依赖输入的采样值与单调时间，便于单元测试。
#[derive(Debug, Clone, Copy)]
struct Engine {
    phase: Phase,
    /// 连续 ≥ 触发线的秒数
    over_secs: u64,
    /// 连续 ≥ 紧急线的秒数
    emergency_secs: u64,
}

impl Engine {
    const fn new() -> Self {
        Engine {
            phase: Phase::Armed,
            over_secs: 0,
            emergency_secs: 0,
        }
    }

    fn reset(&mut self) {
        *self = Engine::new();
    }

    /// 常规触发线：暂停期内需要比低点再上涨 RISE_STEP。
    fn trigger_line(&self, threshold: u32) -> u32 {
        match self.phase {
            Phase::Paused { floor } => threshold.max(floor + RISE_STEP),
            _ => threshold,
        }
    }

    /// 紧急触发线：暂停期内同样要求比低点明显上涨，否则加速后本就稳定在 95% 附近时会反复白做。
    fn emergency_line(&self) -> u32 {
        match self.phase {
            Phase::Paused { floor } => EMERGENCY_PERCENT.max(floor + RISE_STEP),
            _ => EMERGENCY_PERCENT,
        }
    }

    /// 推进一次采样；返回本次是否应触发加速（不含场景守卫）。
    /// 调用方真正执行加速后必须调用 [`Engine::on_boosted`]；被守卫拦下时不调用，
    /// 计数会保留，守卫解除后的下一次采样立即触发。
    fn observe(&mut self, load: u32, threshold: u32, now_ms: i64, tick_secs: u64) -> Option<Lane> {
        if let Phase::Settling { until_ms, boosted_at } = self.phase {
            if now_ms < until_ms {
                return None;
            }
            // 观察期结束：以此时的内存水平判断加速效果
            self.phase = if load + REARM_MARGIN <= threshold {
                // 回落到阈值以下：加速有效
                Phase::Armed
            } else if load >= boosted_at + RISE_STEP {
                // 比加速时还高出一截：是加速之后出现的新需求（例如刚打开大型程序），
                // 不能据此判定加速无效，否则暂停会连紧急处理一起挡住
                Phase::Armed
            } else {
                // 降不下来：程序实际占用
                Phase::Paused { floor: load }
            };
            self.over_secs = 0;
            self.emergency_secs = 0;
        }
        if let Phase::Paused { floor } = self.phase {
            if load + REARM_MARGIN <= threshold {
                // 内存已回落到阈值以下：这一轮高占用结束，恢复正常触发
                self.phase = Phase::Armed;
                self.over_secs = 0;
                self.emergency_secs = 0;
            } else if load < floor {
                // 低点跟随下降：例如用户关掉了部分程序
                self.phase = Phase::Paused { floor: load };
            }
        }
        let line = self.trigger_line(threshold);
        self.over_secs = if load >= line { self.over_secs + tick_secs } else { 0 };
        self.emergency_secs = if load >= self.emergency_line() {
            self.emergency_secs + tick_secs
        } else {
            0
        };
        if self.emergency_secs >= EMERGENCY_SUSTAINED_SECS {
            return Some(Lane::Emergency);
        }
        if self.over_secs >= SUSTAINED_SECS {
            return Some(Lane::Normal);
        }
        None
    }

    /// 已执行加速：进入观察期。`load` 为加速时的内存占用。
    fn on_boosted(&mut self, now_ms: i64, load: u32) {
        self.phase = Phase::Settling {
            until_ms: now_ms + SETTLE_SECS as i64 * 1000,
            boosted_at: load,
        };
        self.over_secs = 0;
        self.emergency_secs = 0;
    }
}

static CONFIG: LazyLock<Mutex<Settings>> = LazyLock::new(|| Mutex::new(Settings::default()));
static ENGINE: Mutex<Engine> = Mutex::new(Engine::new());
static LAST: LazyLock<Mutex<Option<BoostEvent>>> = LazyLock::new(|| Mutex::new(None));

fn settings_path() -> PathBuf {
    crate::history_store::base_dir().join("settings.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) -> Result<(), String> {
    let p = settings_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建设置目录失败：{}", e))?;
    }
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&p, json).map_err(|e| format!("写入设置失败：{}", e))
}

fn clamp_threshold(v: u8) -> u8 {
    v.clamp(THRESHOLD_MIN, THRESHOLD_MAX)
}

// ---------------------------------------------------------------------------
// 场景守卫
// ---------------------------------------------------------------------------

/// 电池供电中。电量状态未知时按交流电处理（不因测量失败而永久暂停）。
fn on_battery() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    unsafe {
        let mut s = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut s).is_ok() {
            s.ACLineStatus == 0
        } else {
            false
        }
    }
}

/// 用户是否处在「不宜打扰」的全屏场景：D3D 独占全屏（游戏）、演示模式，
/// 或前台是**无边框**全屏窗口（浏览器/播放器全屏视频、无边框游戏）。
///
/// 旧实现只比较窗口矩形是否覆盖整个显示器，但任务栏自动隐藏时，普通最大化窗口
/// （浏览器、IDE、截图工具遮罩）也会覆盖整屏，于是自动加速几乎一直被挡住。
/// 现在优先使用 Windows 自身的判断（SHQueryUserNotificationState，与系统
/// 「全屏时不显示通知」同一口径），再补充“覆盖整屏且没有标题栏/边框”的无边框全屏判断。
fn foreground_fullscreen() -> bool {
    use windows::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE,
        QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    if let Ok(state) = unsafe { SHQueryUserNotificationState() } {
        if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
            return true;
        }
        if state != QUNS_BUSY {
            return false;
        }
    }
    borderless_fullscreen_foreground()
}

/// 前台窗口覆盖整个显示器、并且没有标题栏与可调整边框（最大化的普通窗口不算）。
fn borderless_fullscreen_foreground() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GWL_STYLE, WS_CAPTION,
        WS_THICKFRAME,
    };
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        if style & (WS_CAPTION.0 | WS_THICKFRAME.0) != 0 {
            return false;
        }
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
            return false;
        }
        covers_monitor(&wr, &mi.rcMonitor)
    }
}

/// 纯函数：窗口矩形是否覆盖整个显示器。
fn covers_monitor(w: &windows::Win32::Foundation::RECT, m: &windows::Win32::Foundation::RECT) -> bool {
    w.left <= m.left && w.top <= m.top && w.right >= m.right && w.bottom >= m.bottom
}

/// 磁盘总繁忙度（% Disk Time，采样 800ms）。测量失败返回 None（视为不忙）。
fn disk_busy_percent() -> Option<f64> {
    use windows::core::w;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
    };
    unsafe {
        let mut query: isize = 0;
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            return None;
        }
        let mut counter: isize = 0;
        if PdhAddEnglishCounterW(query, w!("\\PhysicalDisk(_Total)\\% Disk Time"), 0, &mut counter)
            != 0
        {
            let _ = PdhCloseQuery(query);
            return None;
        }
        let _ = PdhCollectQueryData(query);
        thread::sleep(Duration::from_millis(800));
        let _ = PdhCollectQueryData(query);
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let r = PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut value);
        let _ = PdhCloseQuery(query);
        if r == 0 {
            Some(value.Anonymous.doubleValue)
        } else {
            None
        }
    }
}

/// 启动后台检查线程；读取已持久化的设置（缺省：开启，85%）。
pub fn start(app: tauri::AppHandle) {
    if let Ok(mut cfg) = CONFIG.lock() {
        *cfg = load_settings();
        cfg.auto_boost_threshold = clamp_threshold(cfg.auto_boost_threshold);
    }
    thread::spawn(move || {
        // 观察期用单调时钟计时：系统改时间 / 时区切换不会让状态卡住
        let clock = Instant::now();
        let tick_secs = TICK.as_secs();
        let mut published: Option<(&'static str, Option<u32>)> = None;
        loop {
            thread::sleep(TICK);
            // 主窗口/WebView2 尚未就绪时不裁剪工作集：启动阶段裁剪会放大页面错误。
            if !crate::webview_ready() {
                continue;
            }
            let cfg = match CONFIG.lock() {
                Ok(c) => c.clone(),
                Err(_) => continue,
            };
            if !cfg.auto_boost_enabled {
                if let Ok(mut e) = ENGINE.lock() {
                    e.reset();
                }
                publish_state(&app, &mut published);
                continue;
            }
            let load = memboost::memory_load_percent();
            let threshold = cfg.auto_boost_threshold as u32;
            let now = clock.elapsed().as_millis() as i64;
            let decision = match ENGINE.lock() {
                Ok(mut e) => e.observe(load, threshold, now, tick_secs),
                Err(_) => continue,
            };
            publish_state(&app, &mut published);
            let lane = match decision {
                Some(lane) => lane,
                None => continue,
            };

            // 场景守卫：电池供电两条通道都尊重（笔记本省电优先）；
            // 全屏与磁盘繁忙只约束常规触发——内存将满时的磁盘繁忙正是换页造成的。
            if on_battery() {
                continue;
            }
            if lane == Lane::Normal {
                if foreground_fullscreen() {
                    continue;
                }
                if let Some(disk) = disk_busy_percent() {
                    if disk > DISK_BUSY_PERCENT {
                        continue;
                    }
                }
            }

            let r = memboost::memory_boost();
            if let Ok(mut e) = ENGINE.lock() {
                e.on_boosted(clock.elapsed().as_millis() as i64, load);
            }
            let ev = BoostEvent {
                ts_ms: chrono::Local::now().timestamp_millis(),
                mem_load: load,
                freed_mb: r.freed_mb,
                trimmed_count: r.trimmed_count,
            };
            if let Ok(mut l) = LAST.lock() {
                *l = Some(ev.clone());
            }
            // 窗口可能隐藏到托盘：把结果写到托盘 tooltip，悬停即可看到
            let _ = app.tray_handle().set_tooltip(&last_boost_tooltip(&ev));
            // 低收益时不弹提示（前端 toast 仅在释放量有意义时触发）
            if r.freed_mb >= MEANINGFUL_FREE_MB {
                let _ = app.emit_all("auto-boost", &ev);
            }
            publish_state(&app, &mut published);
        }
    });
}

fn last_boost_tooltip(ev: &BoostEvent) -> String {
    let time = chrono::DateTime::from_timestamp_millis(ev.ts_ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    format!("Win-Top：上次自动加速 {}，释放 {:.0} MB", time, ev.freed_mb)
}

/// 状态变化时推送给前端，并在进入 / 退出暂停时更新托盘提示。
fn publish_state(app: &tauri::AppHandle, published: &mut Option<(&'static str, Option<u32>)>) {
    let state = get_state();
    let key = (state.status, state.paused_floor);
    if published.as_ref() == Some(&key) {
        return;
    }
    let was_paused = matches!(published, Some(("paused", _)));
    *published = Some(key);
    if state.status == "paused" {
        let _ = app.tray_handle().set_tooltip(&format!(
            "Win-Top：加速后内存仍在 {}%，属于程序实际占用，已暂停重复加速",
            state.paused_floor.unwrap_or_default()
        ));
    } else if was_paused {
        let tip = state
            .last
            .as_ref()
            .map(last_boost_tooltip)
            .unwrap_or_else(|| "Win-Top：关闭窗口后在后台运行".to_string());
        let _ = app.tray_handle().set_tooltip(&tip);
    }
    let _ = app.emit_all("auto-boost-state", &state);
}

pub fn get_state() -> AutoBoostState {
    let (enabled, threshold) = match CONFIG.lock() {
        Ok(c) => (c.auto_boost_enabled, c.auto_boost_threshold),
        Err(_) => (true, DEFAULT_THRESHOLD),
    };
    let engine = ENGINE.lock().map(|e| *e).unwrap_or(Engine::new());
    let (status, paused_floor) = match engine.phase {
        Phase::Armed => ("armed", None),
        Phase::Settling { .. } => ("settling", None),
        Phase::Paused { floor } => ("paused", Some(floor)),
    };
    let t = threshold as u32;
    AutoBoostState {
        enabled,
        threshold,
        status,
        paused_floor,
        resume_above: paused_floor.map(|_| engine.trigger_line(t)),
        rearm_at: t.saturating_sub(REARM_MARGIN),
        last: LAST.lock().ok().and_then(|l| l.clone()),
    }
}

pub fn set_config(enabled: bool, threshold: u8) -> Result<AutoBoostState, String> {
    let s = Settings {
        auto_boost_enabled: enabled,
        auto_boost_threshold: clamp_threshold(threshold),
    };
    save_settings(&s)?;
    if let Ok(mut c) = CONFIG.lock() {
        *c = s;
    }
    // 阈值或开关变化后重新开始判断，不沿用旧阈值下的暂停状态
    if let Ok(mut e) = ENGINE.lock() {
        e.reset();
    }
    Ok(get_state())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_clamped() {
        assert_eq!(clamp_threshold(10), THRESHOLD_MIN);
        assert_eq!(clamp_threshold(200), THRESHOLD_MAX);
        assert_eq!(clamp_threshold(85), 85);
    }

    #[test]
    fn settings_json_roundtrip_and_legacy_fields() {
        let s = Settings {
            auto_boost_enabled: false,
            auto_boost_threshold: 90,
        };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!back.auto_boost_enabled);
        assert_eq!(back.auto_boost_threshold, 90);
        // 空 JSON 解析出默认值；旧版本写入的 auto_boost_last_ms 会被忽略
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert!(empty.auto_boost_enabled);
        assert_eq!(empty.auto_boost_threshold, DEFAULT_THRESHOLD);
        let legacy: Settings = serde_json::from_str(
            r#"{"auto_boost_enabled":true,"auto_boost_threshold":70,"auto_boost_last_ms":1795115117811}"#,
        )
        .unwrap();
        assert_eq!(legacy.auto_boost_threshold, 70);
    }

    /// 以 5 秒为步长回放一段内存曲线：`segments` 为（持续秒数, 内存%）。
    /// 返回结束时刻与每次触发的（时刻秒, 通道）；触发后视为已执行加速。
    fn simulate(
        engine: &mut Engine,
        threshold: u32,
        start: u64,
        segments: &[(u64, u32)],
    ) -> (u64, Vec<(u64, Lane)>) {
        let mut t = start;
        let mut fired = Vec::new();
        for &(secs, load) in segments {
            for _ in 0..secs / 5 {
                t += 5;
                if let Some(lane) = engine.observe(load, threshold, (t * 1000) as i64, 5) {
                    fired.push((t, lane));
                    engine.on_boosted((t * 1000) as i64, load);
                }
            }
        }
        (t, fired)
    }

    #[test]
    fn triggers_only_after_sustained_load() {
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 70, 0, &[(40, 75), (5, 60), (40, 75)]);
        assert!(fired.is_empty(), "每段都不足 45 秒，且中途回落");
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 70, 0, &[(45, 75)]);
        assert_eq!(fired, vec![(45, Lane::Normal)]);
    }

    /// 加速有效（回落到阈值以下）后没有冷却等待：内存再次持续超阈值就再次加速。
    #[test]
    fn effective_boost_rearms_without_cooldown() {
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 70, 0, &[(45, 75), (30, 55), (120, 72)]);
        assert_eq!(fired, vec![(45, Lane::Normal), (120, Lane::Normal)]);
        assert!(fired[1].0 - fired[0].0 < 600, "旧逻辑要等 10 分钟");
    }

    /// 用户描述的场景：程序实际占用 80%，阈值 70%，再怎么加速也降不下来——不能反复加速。
    #[test]
    fn persistent_usage_pauses_repeated_boosts() {
        let mut e = Engine::new();
        let (t, fired) = simulate(&mut e, 70, 0, &[(45, 82), (30, 79), (3600, 80)]);
        assert_eq!(fired, vec![(45, Lane::Normal)], "一小时内只加速一次");
        assert_eq!(e.phase, Phase::Paused { floor: 79 });
        // 比加速后的低点再上涨 5 个百分点 → 又积累了可释放内存，再加速一次
        let (_, fired) = simulate(&mut e, 70, t, &[(60, 84)]);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].1, Lane::Normal);
    }

    #[test]
    fn pause_ends_when_memory_drops_below_threshold() {
        let mut e = Engine::new();
        let (t, _) = simulate(&mut e, 70, 0, &[(45, 82), (30, 79)]);
        assert!(matches!(e.phase, Phase::Paused { .. }));
        let (t, _) = simulate(&mut e, 70, t, &[(10, 66)]);
        assert_eq!(e.phase, Phase::Armed);
        let (_, fired) = simulate(&mut e, 70, t, &[(45, 72)]);
        assert_eq!(fired.len(), 1, "回落后再次超阈值，按正常规则触发");
    }

    #[test]
    fn floor_follows_memory_down() {
        let mut e = Engine::new();
        let (t, _) = simulate(&mut e, 70, 0, &[(45, 90), (30, 92), (60, 84)]);
        assert_eq!(e.phase, Phase::Paused { floor: 84 }, "用户关闭了部分程序");
        let (_, fired) = simulate(&mut e, 70, t, &[(45, 89)]);
        assert_eq!(fired.len(), 1, "比新的低点上涨 5 个百分点即再次加速");
    }

    /// 加速后刚好停在阈值附近（68%，阈值 70%）不算回落，避免在阈值边缘反复加速。
    #[test]
    fn hysteresis_around_threshold() {
        let mut e = Engine::new();
        let (t, _) = simulate(&mut e, 70, 0, &[(45, 75), (30, 68)]);
        assert_eq!(e.phase, Phase::Paused { floor: 68 });
        let (t, fired) = simulate(&mut e, 70, t, &[(300, 71)]);
        assert!(fired.is_empty(), "71% 只比低点高 3 个百分点，不重复加速");
        simulate(&mut e, 70, t, &[(5, 67)]);
        assert_eq!(e.phase, Phase::Armed);
    }

    #[test]
    fn settling_blocks_immediate_retrigger() {
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 70, 0, &[(45, 75), (25, 99)]);
        assert_eq!(fired, vec![(45, Lane::Normal)], "观察期内不再触发");
    }

    /// 加速后观察期内出现新的尖峰（72% 加速，随后打开大型程序冲到 99%）：
    /// 不能误判为「加速无效」而暂停，紧急处理必须照常进行。
    #[test]
    fn spike_after_boost_is_not_misread_as_ineffective() {
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 70, 0, &[(45, 72), (15, 70), (20, 99)]);
        assert_eq!(fired, vec![(45, Lane::Normal), (80, Lane::Emergency)]);
    }

    /// 内存持续爬升时，每次加速后都比加速时更高：允许继续加速，但次数受上涨幅度约束。
    #[test]
    fn climbing_memory_keeps_boosting_until_it_plateaus() {
        let mut e = Engine::new();
        let (_, fired) = simulate(
            &mut e,
            70,
            0,
            &[(45, 72), (30, 78), (45, 78), (30, 84), (45, 84), (30, 86), (600, 86)],
        );
        assert_eq!(fired.len(), 3, "72→78→84 每轮上涨 ≥5 个百分点；86 不再重复");
        assert!(matches!(e.phase, Phase::Paused { floor: 86 }));
    }

    #[test]
    fn emergency_fires_fast() {
        let mut e = Engine::new();
        let (_, fired) = simulate(&mut e, 85, 0, &[(10, 97)]);
        assert_eq!(fired, vec![(10, Lane::Emergency)]);
    }

    /// 复现 11-20 03:44：上一次加速有效，随后 20 秒内内存从 80% 冲到 99%。
    #[test]
    fn spike_after_recent_boost_is_handled() {
        let mut e = Engine::new();
        let (t, _) = simulate(&mut e, 70, 0, &[(45, 76), (30, 58), (90, 62)]);
        assert_eq!(e.phase, Phase::Armed);
        let (_, fired) = simulate(
            &mut e,
            70,
            t,
            &[(5, 80), (5, 83), (5, 86), (5, 90), (5, 94), (5, 96), (5, 97), (5, 98)],
        );
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0], (t + 35, Lane::Emergency), "达到 95% 后 10 秒内处理");
    }

    #[test]
    fn emergency_during_pause_requires_real_rise() {
        let mut e = Engine::new();
        let (t, _) = simulate(&mut e, 70, 0, &[(45, 82), (30, 80)]);
        let (_, fired) = simulate(&mut e, 70, t, &[(10, 96)]);
        assert_eq!(fired, vec![(t + 10, Lane::Emergency)], "从 80% 冲到 96%，仍要紧急处理");

        // 加速后本身就稳定在 97%：再加速也无济于事，不再反复执行
        let mut e = Engine::new();
        let (t, fired) = simulate(&mut e, 70, 0, &[(10, 97), (30, 97)]);
        assert_eq!(fired.len(), 1);
        assert_eq!(e.phase, Phase::Paused { floor: 97 });
        let (_, fired) = simulate(&mut e, 70, t, &[(600, 97)]);
        assert!(fired.is_empty());
    }

    /// 被场景守卫拦下时不调用 on_boosted：计数保留，守卫解除后立即触发。
    #[test]
    fn guard_blocked_trigger_stays_pending() {
        let mut e = Engine::new();
        for i in 1..=9 {
            let r = e.observe(80, 70, i * 5000, 5);
            assert_eq!(r.is_some(), i == 9);
        }
        assert_eq!(e.observe(80, 70, 50_000, 5), Some(Lane::Normal));
        e.on_boosted(50_000, 80);
        assert!(matches!(e.phase, Phase::Settling { .. }));
    }

    #[test]
    fn reset_returns_to_armed() {
        let mut e = Engine::new();
        simulate(&mut e, 70, 0, &[(45, 82), (30, 79)]);
        e.reset();
        assert_eq!(e.phase, Phase::Armed);
        assert_eq!(e.over_secs, 0);
    }

    #[test]
    fn maximized_window_is_not_fullscreen_by_rect_alone() {
        use windows::Win32::Foundation::RECT;
        let monitor = RECT { left: 0, top: 0, right: 2194, bottom: 1234 };
        let full = RECT { left: 0, top: 0, right: 2194, bottom: 1234 };
        let windowed = RECT { left: 10, top: 10, right: 1800, bottom: 1100 };
        assert!(covers_monitor(&full, &monitor));
        assert!(!covers_monitor(&windowed, &monitor));
    }
}
