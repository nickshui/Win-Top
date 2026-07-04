//! 定时任务调度器：支持按间隔触发后台任务。
//! 当前仅支持垃圾清理扫描通知。不持久化（重启失效）。

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

use tauri::Manager;

pub struct Scheduler {
    running: Arc<AtomicBool>,
    interval_hours: Arc<Mutex<u64>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler {
            running: Arc::new(AtomicBool::new(false)),
            interval_hours: Arc::new(Mutex::new(24)),
        }
    }
    
    pub fn start(&self, app_handle: tauri::AppHandle) {
        let running = self.running.clone();
        let interval = self.interval_hours.clone();
        running.store(true, Ordering::Relaxed);
        
        thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                let hrs = *interval.lock().unwrap();
                thread::sleep(Duration::from_secs(hrs * 3600));
                if !running.load(Ordering::Relaxed) { break; }
                
                // Run scan and emit notification
                let report = crate::cleanup::scan_junk();
                let total_mb = report.total_bytes as f64 / 1024.0 / 1024.0;
                let _ = app_handle.emit_all("scheduled-cleanup", serde_json::json!({
                    "total_mb": total_mb,
                    "categories": report.categories.iter().map(|c| serde_json::json!({
                        "label": c.label,
                        "mb": c.bytes as f64 / 1024.0 / 1024.0
                    })).collect::<Vec<_>>()
                }));
            }
        });
    }
    
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
    
    pub fn set_interval(&self, hours: u64) {
        if let Ok(mut i) = self.interval_hours.lock() {
            *i = hours.max(1);
        }
    }
}

// Global scheduler instance
use std::sync::LazyLock;
static SCHEDULER: LazyLock<Mutex<Option<Scheduler>>> = LazyLock::new(|| Mutex::new(None));

pub fn init_scheduler(app: tauri::AppHandle) {
    let sched = Scheduler::new();
    sched.start(app);
    if let Ok(mut s) = SCHEDULER.lock() {
        *s = Some(sched);
    }
}

pub fn set_schedule_interval(hours: u64) -> Result<String, String> {
    if let Ok(s) = SCHEDULER.lock() {
        if let Some(ref sched) = *s {
            sched.set_interval(hours);
            Ok(format!("清理间隔已设为 {} 小时", hours))
        } else {
            Err("调度器未初始化".to_string())
        }
    } else {
        Err("调度器锁异常".to_string())
    }
}
