#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;

#[derive(Serialize)]
struct MonitorOverviewItem {
    label: String,
    value: f32,
    display: String,
}

#[derive(Serialize)]
struct MonitorSnapshot {
    updated_at: String,
    overview: Vec<MonitorOverviewItem>,
}

#[tauri::command]
fn get_monitor_snapshot() -> MonitorSnapshot {
    MonitorSnapshot {
        updated_at: "Tauri Stub".to_string(),
        overview: vec![
            MonitorOverviewItem {
                label: "CPU 负载".to_string(),
                value: 0.31,
                display: "31%".to_string(),
            },
            MonitorOverviewItem {
                label: "内存压力".to_string(),
                value: 0.58,
                display: "58%".to_string(),
            },
            MonitorOverviewItem {
                label: "磁盘活跃度".to_string(),
                value: 0.46,
                display: "46%".to_string(),
            },
            MonitorOverviewItem {
                label: "网络占用".to_string(),
                value: 0.29,
                display: "29%".to_string(),
            },
        ],
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_monitor_snapshot])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}
