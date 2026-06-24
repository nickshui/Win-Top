#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Win-Top 后端（新架构）
//! 常驻采集器(FastTier) -> Tauri 事件推送；进程等模块走按需 command。
//! 后续扩展：网络端口、磁盘 SMART、ETW 事件流。

#[cfg(target_os = "windows")]
mod collector;
#[cfg(target_os = "windows")]
mod disk;
#[cfg(target_os = "windows")]
mod events;
#[cfg(target_os = "windows")]
mod network;
#[cfg(target_os = "windows")]
mod privilege;
#[cfg(target_os = "windows")]
mod process;
#[cfg(target_os = "windows")]
mod nettraffic;
#[cfg(target_os = "windows")]
mod cleanup;
#[cfg(target_os = "windows")]
mod memboost;

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_processes() -> Result<Vec<process::ProcessRow>, String> {
    process::list_processes()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn terminate_process(pid: u32) -> process::ActionResult {
    process::terminate(pid)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_process_priority(pid: u32, level: String) -> process::ActionResult {
    process::set_priority(pid, &level)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_connections() -> Result<Vec<network::PortRow>, String> {
    network::list_connections()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_disk_report() -> disk::DiskReport {
    disk::report()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn network_checkup() -> network::NetCheckup {
    network::checkup()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn probe_target(input: String) -> network::TargetProbe {
    network::probe_target(&input)
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn speed_test() -> network::SpeedResult {
    // 在阻塞线程池执行，避免占用主线程导致窗口「未响应」
    tauri::async_runtime::spawn_blocking(network::speed_test)
        .await
        .unwrap_or_else(|_| network::SpeedResult {
            down_mbps: 0.0,
            bytes: 0,
            secs: 0.0,
            streams: 0,
            error: Some("测速任务异常退出".to_string()),
        })
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn is_elevated() -> bool {
    privilege::is_elevated()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn relaunch_as_admin() -> Result<(), String> {
    privilege::relaunch_as_admin()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_etw_status() -> events::EtwStatusInfo {
    events::status()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_nettraffic_status() -> nettraffic::NetTrafficStatus {
    nettraffic::status()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn scan_junk() -> cleanup::CleanupReport {
    tauri::async_runtime::spawn_blocking(cleanup::scan_junk)
        .await
        .unwrap_or_else(|_| cleanup::CleanupReport { categories: vec![], total_bytes: 0 })
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn clean_junk(ids: Vec<String>) -> cleanup::CleanupResult {
    tauri::async_runtime::spawn_blocking(move || cleanup::clean_junk(ids))
        .await
        .unwrap_or_else(|_| cleanup::CleanupResult { freed_bytes: 0, items: vec![] })
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn memory_boost() -> memboost::BoostResult {
    tauri::async_runtime::spawn_blocking(memboost::memory_boost)
        .await
        .unwrap_or_else(|_| memboost::BoostResult {
            freed_mb: 0.0,
            trimmed_count: 0,
            before_avail_mb: 0.0,
            after_avail_mb: 0.0,
        })
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn suggest_background() -> Vec<memboost::BgProc> {
    memboost::suggest_background()
}

#[cfg(target_os = "windows")]
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            std::thread::spawn({
                let h = handle.clone();
                move || collector::run_metrics_loop(h)
            });
            // NetEventTier：ETW 每进程网络流量（未提权会报告 net-traffic-status=false）
            nettraffic::start(handle.clone());
            // EventTier：ETW 实时进程事件（未提权会自行报告 etw-status=false）
            events::start(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_processes,
            terminate_process,
            set_process_priority,
            get_connections,
            get_disk_report,
            network_checkup,
            probe_target,
            speed_test,
            is_elevated,
            relaunch_as_admin,
            get_etw_status,
            get_nettraffic_status,
            scan_junk,
            clean_junk,
            memory_boost,
            suggest_background
        ])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Win-Top 仅支持 Windows 运行。");
}
