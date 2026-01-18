#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Local;
use serde::Serialize;
use sysinfo::{DiskExt, NetworkExt, ProcessExt, System, SystemExt};

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

#[derive(Serialize)]
struct ProcessOverviewItem {
    name: String,
    cpu: f32,
    memory: String,
}

#[tauri::command]
fn get_monitor_snapshot() -> MonitorSnapshot {
    let mut system = System::new_all();
    system.refresh_all();

    let cpu_usage = system.global_cpu_info().cpu_usage();
    let total_memory = system.total_memory() as f32;
    let used_memory = system.used_memory() as f32;
    let memory_usage = if total_memory > 0.0 {
        (used_memory / total_memory) * 100.0
    } else {
        0.0
    };

    let (total_disk, available_disk) = system
        .disks()
        .iter()
        .fold((0u64, 0u64), |acc, disk| {
            (acc.0 + disk.total_space(), acc.1 + disk.available_space())
        });
    let disk_usage = if total_disk > 0 {
        ((total_disk - available_disk) as f32 / total_disk as f32) * 100.0
    } else {
        0.0
    };

    let total_network_bytes: u64 = system
        .networks()
        .iter()
        .map(|(_, data)| data.received() + data.transmitted())
        .sum();
    let network_usage = ((total_network_bytes as f32 / 100_000_000.0) * 100.0).min(100.0);

    MonitorSnapshot {
        updated_at: Local::now().format("%H:%M:%S").to_string(),
        overview: vec![
            MonitorOverviewItem {
                label: "CPU 负载".to_string(),
                value: (cpu_usage / 100.0).min(1.0),
                display: format!("{:.0}%", cpu_usage),
            },
            MonitorOverviewItem {
                label: "内存压力".to_string(),
                value: (memory_usage / 100.0).min(1.0),
                display: format!("{:.0}%", memory_usage),
            },
            MonitorOverviewItem {
                label: "磁盘活跃度".to_string(),
                value: (disk_usage / 100.0).min(1.0),
                display: format!("{:.0}%", disk_usage),
            },
            MonitorOverviewItem {
                label: "网络占用".to_string(),
                value: (network_usage / 100.0).min(1.0),
                display: format!("{:.0}%", network_usage),
            },
        ],
    }
}

#[tauri::command]
fn get_process_overview() -> Vec<ProcessOverviewItem> {
    let mut system = System::new_all();
    system.refresh_all();

    let mut processes: Vec<ProcessOverviewItem> = system
        .processes()
        .values()
        .map(|process| ProcessOverviewItem {
            name: process.name().to_string(),
            cpu: process.cpu_usage(),
            memory: format!("{:.1} MB", process.memory() as f32 / 1024.0),
        })
        .collect();

    processes.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    processes.truncate(5);
    processes
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_monitor_snapshot,
            get_process_overview
        ])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}
