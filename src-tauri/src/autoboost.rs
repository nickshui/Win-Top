//! 自动加速：内存占用持续高于阈值时自动裁剪各进程工作集（仅释放内存，不删任何文件）。
//!
//! 策略：
//! * 持续性：连续 3 次采样（45 秒）都达到阈值才触发，避免瞬时尖峰误触发；
//! * 冷却：触发后至少 10 分钟内不重复；时间戳持久化到 settings.json，重启应用不会绕过；
//! * 退避：连续 2 次释放量低于 50 MB 时冷却延长到 30 分钟，出现有效释放后恢复正常；
//! * 守卫：电池供电、前台全屏、磁盘繁忙（>80% Disk Time）时暂停，避免在游戏/会议中制造缺页卡顿。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::memboost;

pub const DEFAULT_THRESHOLD: u8 = 85;
pub const THRESHOLD_MIN: u8 = 50;
pub const THRESHOLD_MAX: u8 = 95;
/// 基础冷却：触发后 10 分钟内不重复。
pub const COOLDOWN_SECS: u64 = 600;
/// 连续低收益后的延长冷却。
pub const COOLDOWN_LOW_GAIN_SECS: u64 = 1800;
/// 连续低收益达到该次数后进入延长冷却。
const LOW_GAIN_STREAK_FOR_BACKOFF: u32 = 2;
/// 一次自动加速至少释放这么多内存才算"有效"。
const MEANINGFUL_FREE_MB: f64 = 50.0;
/// 连续多少次采样达到阈值才触发（15s × 3 = 45s）。
const SUSTAINED_SAMPLES: u32 = 3;
/// 磁盘 % Disk Time 超过该值视为繁忙，暂停自动加速。
const DISK_BUSY_PERCENT: f64 = 80.0;

const TICK: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_enabled")]
    pub auto_boost_enabled: bool,
    #[serde(default = "default_threshold")]
    pub auto_boost_threshold: u8,
    /// 上次自动加速的本地时间戳（毫秒；0 = 从未触发）。持久化以跨重启遵守冷却。
    #[serde(default)]
    pub auto_boost_last_ms: i64,
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
            auto_boost_last_ms: 0,
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

#[derive(Serialize)]
pub struct AutoBoostState {
    pub enabled: bool,
    pub threshold: u8,
    pub cooldown_secs: u64,
    pub last: Option<BoostEvent>,
}

static CONFIG: LazyLock<Mutex<Settings>> = LazyLock::new(|| Mutex::new(Settings::default()));
static LAST: LazyLock<Mutex<Option<BoostEvent>>> = LazyLock::new(|| Mutex::new(None));
/// 连续低收益次数（运行期状态，不持久化）。
static LOW_GAIN_STREAK: AtomicU32 = AtomicU32::new(0);

/// 当前生效的冷却时长：连续低收益时延长，出现有效释放后恢复。
fn effective_cooldown_secs(low_gain_streak: u32) -> u64 {
    if low_gain_streak >= LOW_GAIN_STREAK_FOR_BACKOFF {
        COOLDOWN_LOW_GAIN_SECS
    } else {
        COOLDOWN_SECS
    }
}

/// 纯函数：根据本次释放量更新低收益连续计数。
fn next_low_gain_streak(prev: u32, freed_mb: f64) -> u32 {
    if freed_mb < MEANINGFUL_FREE_MB {
        prev.saturating_add(1)
    } else {
        0
    }
}

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

/// 前台窗口是否全屏（无边框/独占全屏；排除桌面与任务栏窗口）。
fn foreground_fullscreen() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowRect,
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
        let m = mi.rcMonitor;
        wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom
    }
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
        let mut over_samples: u32 = 0;
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
                over_samples = 0;
                continue;
            }
            let load = memboost::memory_load_percent();
            if load < cfg.auto_boost_threshold as u32 {
                over_samples = 0;
                continue;
            }
            // 持续性：连续多次采样超阈值才触发
            over_samples += 1;
            if over_samples < SUSTAINED_SAMPLES {
                continue;
            }
            // 冷却：时间戳持久化（跨重启生效）；时钟被回拨（elapsed < 0）时按已到期处理
            let now_ms = chrono::Local::now().timestamp_millis();
            let cooldown_ms =
                effective_cooldown_secs(LOW_GAIN_STREAK.load(Ordering::Relaxed)) as i64 * 1000;
            let elapsed = now_ms - cfg.auto_boost_last_ms;
            if cfg.auto_boost_last_ms != 0 && elapsed >= 0 && elapsed < cooldown_ms {
                continue;
            }
            // 场景守卫：电池 / 前台全屏 / 磁盘繁忙时暂停
            if on_battery() {
                continue;
            }
            if foreground_fullscreen() {
                continue;
            }
            if let Some(disk) = disk_busy_percent() {
                if disk > DISK_BUSY_PERCENT {
                    continue;
                }
            }

            // 触发
            over_samples = 0;
            let r = memboost::memory_boost();
            let streak =
                next_low_gain_streak(LOW_GAIN_STREAK.load(Ordering::Relaxed), r.freed_mb);
            LOW_GAIN_STREAK.store(streak, Ordering::Relaxed);

            // 持久化最近触发时间（跨重启遵守冷却）
            if let Ok(mut c) = CONFIG.lock() {
                c.auto_boost_last_ms = now_ms;
            }
            if let Ok(c) = CONFIG.lock() {
                let _ = save_settings(&c);
            }

            let ev = BoostEvent {
                ts_ms: now_ms,
                mem_load: load,
                freed_mb: r.freed_mb,
                trimmed_count: r.trimmed_count,
            };
            if let Ok(mut l) = LAST.lock() {
                *l = Some(ev.clone());
            }
            // 窗口可能隐藏到托盘：把结果写到托盘 tooltip，悬停即可看到
            let time_str = chrono::Local::now().format("%H:%M");
            let _ = app.tray_handle().set_tooltip(&format!(
                "Win-Top：上次自动加速 {}，释放 {:.0} MB",
                time_str, ev.freed_mb
            ));
            // 低收益时不弹提示（前端 toast 仅在释放量有意义时触发）
            if r.freed_mb >= MEANINGFUL_FREE_MB {
                let _ = app.emit_all("auto-boost", &ev);
            }
        }
    });
}

pub fn get_state() -> AutoBoostState {
    let (enabled, threshold) = match CONFIG.lock() {
        Ok(c) => (c.auto_boost_enabled, c.auto_boost_threshold),
        Err(_) => (true, DEFAULT_THRESHOLD),
    };
    let last = LAST.lock().ok().and_then(|l| l.clone());
    AutoBoostState {
        enabled,
        threshold,
        cooldown_secs: effective_cooldown_secs(LOW_GAIN_STREAK.load(Ordering::Relaxed)),
        last,
    }
}

pub fn set_config(enabled: bool, threshold: u8) -> Result<AutoBoostState, String> {
    // 保留已持久化的最近触发时间，避免改阈值把冷却重置
    let mut s = match CONFIG.lock() {
        Ok(c) => c.clone(),
        Err(_) => Settings::default(),
    };
    s.auto_boost_enabled = enabled;
    s.auto_boost_threshold = clamp_threshold(threshold);
    save_settings(&s)?;
    if let Ok(mut c) = CONFIG.lock() {
        *c = s;
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
    fn settings_json_roundtrip() {
        let s = Settings {
            auto_boost_enabled: false,
            auto_boost_threshold: 90,
            auto_boost_last_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(!back.auto_boost_enabled);
        assert_eq!(back.auto_boost_threshold, 90);
        assert_eq!(back.auto_boost_last_ms, 1_700_000_000_000);
    }

    #[test]
    fn defaults_are_on_85() {
        let s = Settings::default();
        assert!(s.auto_boost_enabled);
        assert_eq!(s.auto_boost_threshold, DEFAULT_THRESHOLD);
        assert_eq!(s.auto_boost_last_ms, 0);
        // 空 JSON 也能解析出默认值（含旧版 settings.json 缺字段的兼容）
        let back: Settings = serde_json::from_str("{}").unwrap();
        assert!(back.auto_boost_enabled);
        assert_eq!(back.auto_boost_threshold, DEFAULT_THRESHOLD);
        assert_eq!(back.auto_boost_last_ms, 0);
        let old: Settings =
            serde_json::from_str("{\"auto_boost_enabled\":false,\"auto_boost_threshold\":70}")
                .unwrap();
        assert!(!old.auto_boost_enabled);
        assert_eq!(old.auto_boost_threshold, 70);
        assert_eq!(old.auto_boost_last_ms, 0);
    }

    #[test]
    fn cooldown_extends_after_low_gain_streak() {
        assert_eq!(effective_cooldown_secs(0), COOLDOWN_SECS);
        assert_eq!(effective_cooldown_secs(1), COOLDOWN_SECS);
        assert_eq!(effective_cooldown_secs(2), COOLDOWN_LOW_GAIN_SECS);
        assert_eq!(effective_cooldown_secs(9), COOLDOWN_LOW_GAIN_SECS);
    }

    #[test]
    fn low_gain_streak_tracking() {
        // 低于 50 MB 记一次低收益
        assert_eq!(next_low_gain_streak(0, 10.0), 1);
        assert_eq!(next_low_gain_streak(1, 49.9), 2);
        // 有效释放后清零
        assert_eq!(next_low_gain_streak(3, 50.0), 0);
        assert_eq!(next_low_gain_streak(5, 1000.0), 0);
        // 饱和不溢出
        assert_eq!(next_low_gain_streak(u32::MAX, 0.0), u32::MAX);
    }
}
