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
fn speed_test() -> network::SpeedResult {
    network::speed_test()
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
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle();
            std::thread::spawn({
                let h = handle.clone();
                move || collector::run_metrics_loop(h)
            });
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
            get_etw_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Win-Top 仅支持 Windows 运行。");
}
