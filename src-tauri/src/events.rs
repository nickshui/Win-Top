//! ETW 实时事件流（EventTier）：订阅 Microsoft-Windows-Kernel-Process，
//! 实时推送进程启停事件给前端。需要管理员权限（实时 ETW 会话）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::{stop_trace_by_name, UserTrace};
use ferrisetw::EventRecord;

const KERNEL_PROCESS_GUID: &str = "22FB2CD6-0E7B-422B-A0C7-2FAD1FD0E716";
const SESSION_NAME: &str = "WinTopProcTrace";

// ETW 状态共享：避免「后端在前端监听器注册前 emit 状态导致前端收不到」的时序竞争。
static ETW_OK: AtomicBool = AtomicBool::new(false);
static ETW_REASON: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new("初始化中…".to_string()));

#[derive(Serialize, Clone)]
pub struct EtwStatusInfo {
    pub available: bool,
    pub reason: String,
}

pub fn status() -> EtwStatusInfo {
    EtwStatusInfo {
        available: ETW_OK.load(Ordering::Relaxed),
        reason: ETW_REASON.lock().map(|s| s.clone()).unwrap_or_default(),
    }
}

fn set_status(app: &AppHandle, available: bool, reason: &str) {
    ETW_OK.store(available, Ordering::Relaxed);
    if let Ok(mut r) = ETW_REASON.lock() {
        *r = reason.to_string();
    }
    let _ = app.emit_all(
        "etw-status",
        EtwStatusInfo {
            available,
            reason: reason.to_string(),
        },
    );
}

#[derive(Serialize, Clone)]
struct ProcEvent {
    ts: String,
    action: String, // start / stop
    pid: u32,
    image: String,
}

fn basename(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// 在后台线程启动 ETW 会话。未提权时会失败，并通过状态/事件告知前端。
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let cb_app = app.clone();
        let provider = Provider::by_guid(KERNEL_PROCESS_GUID)
            .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
                if let Ok(schema) = locator.event_schema(record) {
                    let action = match record.event_id() {
                        1 => "start",
                        2 => "stop",
                        _ => return,
                    };
                    let parser = Parser::create(record, &schema);
                    let pid: u32 = parser.try_parse("ProcessID").unwrap_or(0);
                    let image: String = parser.try_parse("ImageName").unwrap_or_default();
                    let _ = cb_app.emit_all(
                        "proc-event",
                        ProcEvent {
                            ts: Local::now().format("%H:%M:%S").to_string(),
                            action: action.to_string(),
                            pid,
                            image: basename(&image),
                        },
                    );
                }
            })
            .build();

        // 实时 ETW 会话在内核持久化：被强制关闭的旧实例会残留同名会话，
        // 导致本次创建失败。固定名 + 启动前回收 => 泄漏永远 ≤1 个且每次自动回收。
        let _ = stop_trace_by_name(SESSION_NAME);

        match UserTrace::new()
            .named(SESSION_NAME.to_string())
            .enable(provider)
            .start_and_process()
        {
            Ok(_trace) => {
                set_status(&app, true, "");
                // 保持 trace 存活：drop 会停止 ETW 会话
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            Err(e) => {
                set_status(&app, false, &format!("ETW 会话启动失败：{:?}", e));
            }
        }
    });
}
