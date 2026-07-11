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
mod history;
#[cfg(target_os = "windows")]
mod memboost;
#[cfg(target_os = "windows")]
mod startup;
#[cfg(target_os = "windows")]
mod export;
#[cfg(target_os = "windows")]
mod scheduler;
#[cfg(target_os = "windows")]
mod gpu;
#[cfg(target_os = "windows")]
mod geoip;
#[cfg(target_os = "windows")]
mod firewall;
#[cfg(target_os = "windows")]
mod restore;
#[cfg(target_os = "windows")]
mod file_unlock;
#[cfg(target_os = "windows")]
mod disk_usage;
#[cfg(target_os = "windows")]
mod mft_scan;
#[cfg(target_os = "windows")]
mod disk_io;
#[cfg(target_os = "windows")]
mod health_check;
#[cfg(target_os = "windows")]
mod services;
#[cfg(target_os = "windows")]
mod visual_effects;

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_gpus() -> Result<Vec<gpu::GpuInfo>, String> {
    gpu::list_gpus()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn geo_lookup(ip: String) -> Option<geoip::GeoInfo> {
    geoip::lookup(&ip)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn list_firewall_rules() -> Result<Vec<firewall::FirewallRule>, String> {
    firewall::list_rules()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn toggle_firewall_rule(name: String, enabled: bool) -> Result<String, String> {
    firewall::toggle_rule(&name, enabled)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn list_restore_points() -> Result<Vec<restore::RestorePoint>, String> {
    restore::list_restore_points()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn create_restore_point(description: String) -> Result<String, String> {
    restore::create_restore_point(&description)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn find_file_locks(file_path: String) -> Result<Vec<file_unlock::FileLockInfo>, String> {
    file_unlock::find_locks(&file_path)
}

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
fn get_process_detail(pid: u32) -> Result<process::ProcessDetail, String> {
    process::get_process_detail(pid)
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
async fn scan_directory(dir_path: String, top_n: usize) -> disk_usage::UsageReport {
    disk_usage::reset_cancel();
    tauri::async_runtime::spawn_blocking(move || disk_usage::scan_directory(dir_path, top_n))
        .await
        .unwrap_or_else(|_| disk_usage::UsageReport {
            large_files: vec![],
            dirs: vec![],
            scanned: 0,
            errors: 0,
            source: "walk".into(),
            elapsed_ms: 0,
            cancelled: false,
        })
}

/// 按盘符扫描整卷：优先走 MFT（需管理员、仅 NTFS），失败时回退到逐目录递归。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn scan_volume(drive: String, top_n: usize) -> disk_usage::UsageReport {
    disk_usage::reset_cancel();
    tauri::async_runtime::spawn_blocking(move || {
        match mft_scan::scan_volume(&drive, top_n) {
            Ok(report) => report,
            Err(_) => {
                // 回退：从盘根递归遍历（慢，但不依赖 MFT/权限）
                let root = format!("{}\\", drive.trim().trim_end_matches('\\').trim_end_matches(':'));
                disk_usage::scan_directory(root, top_n)
            }
        }
    })
    .await
    .unwrap_or_else(|_| disk_usage::UsageReport {
        large_files: vec![],
        dirs: vec![],
        scanned: 0,
        errors: 0,
        source: "walk".into(),
        elapsed_ms: 0,
        cancelled: false,
    })
}

/// 请求取消正在进行的空间分析扫描（整卷或下钻）。
/// 扫描线程会在下一个检查点尽快停止，并返回取消前已收集到的部分结果。
#[cfg(target_os = "windows")]
#[tauri::command]
fn cancel_scan() {
    disk_usage::request_cancel();
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
#[tauri::command]
fn list_startup() -> Vec<startup::StartupItem> {
    startup::list_startup()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_startup_enabled(id: String, enabled: bool) -> process::ActionResult {
    startup::set_startup_enabled(id, enabled)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_history(n: usize) -> Vec<collector::MetricsSnapshot> {
    history::recent(n)
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn export_snapshot() -> Result<export::ExportResult, String> {
    tauri::async_runtime::spawn_blocking(export::export_snapshot)
        .await
        .unwrap_or_else(|_| Err("导出任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn export_processes_csv() -> Result<export::ExportResult, String> {
    tauri::async_runtime::spawn_blocking(export::export_processes_csv)
        .await
        .unwrap_or_else(|_| Err("导出任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_cleanup_schedule(hours: u64) -> Result<String, String> {
    scheduler::set_schedule_interval(hours)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn run_health_check() -> Result<health_check::HealthReport, String> {
    health_check::run_health_check()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn list_services() -> Result<Vec<services::ServiceInfo>, String> {
    services::list_non_ms_services()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn set_service_startup(name: String, startup_type: String) -> services::ServiceActionResult {
    tauri::async_runtime::spawn_blocking(move || services::set_service_startup(&name, &startup_type))
        .await
        .unwrap_or_else(|_| services::ServiceActionResult { success: false, message: "操作异常退出".into(), can_rollback: false })
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn restore_service(name: String) -> services::ServiceActionResult {
    services::restore_service(&name)
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn restore_all_services() -> services::ServiceActionResult {
    services::restore_all_services()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn list_modified_services() -> Vec<(String, String)> {
    services::list_modified_services()
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_visual_fx_state() -> Result<visual_effects::VisualFxState, String> {
    visual_effects::get_current_state()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn apply_visual_fx_preset(preset: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || visual_effects::apply_preset(&preset))
        .await
        .unwrap_or_else(|_| Err("视觉效果切换任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn restore_visual_fx() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(visual_effects::restore_defaults)
        .await
        .unwrap_or_else(|_| Err("视觉效果还原任务异常退出".to_string()))
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
            events::start(handle.clone());
            // 每进程磁盘 I/O 追踪（低权限，每 2 秒采集）
            disk_io::start(handle.clone());
            // 初始化定时清理调度器
            scheduler::init_scheduler(handle.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_processes,
            terminate_process,
            set_process_priority,
            get_process_detail,
            get_connections,
            get_disk_report,
            scan_directory,
            scan_volume,
            cancel_scan,
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
            suggest_background,
            list_startup,
            set_startup_enabled,
            get_history,
            export_snapshot,
            export_processes_csv,
            set_cleanup_schedule,
            get_gpus,
            geo_lookup,
            list_firewall_rules,
            toggle_firewall_rule,
            list_restore_points,
            create_restore_point,
            find_file_locks,
            run_health_check,
            list_services,
            set_service_startup,
            restore_service,
            restore_all_services,
            list_modified_services,
            get_visual_fx_state,
            apply_visual_fx_preset,
            restore_visual_fx
        ])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Win-Top 仅支持 Windows 运行。");
}
