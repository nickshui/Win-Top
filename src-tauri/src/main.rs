#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Win-Top 后端（新架构）
//! 常驻采集器(FastTier) -> Tauri 事件推送；进程等模块走按需 command。
//! 后续扩展：网络端口、磁盘 SMART、ETW 事件流。

#[cfg(target_os = "windows")]
mod autoboost;
#[cfg(target_os = "windows")]
mod app_icon;
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
mod history_store;
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
mod gpu_proc;
#[cfg(target_os = "windows")]
mod health_check;
#[cfg(target_os = "windows")]
mod services;
#[cfg(target_os = "windows")]
mod visual_effects;
#[cfg(target_os = "windows")]
mod app_inventory;
#[cfg(target_os = "windows")]
mod footprint;
#[cfg(target_os = "windows")]
mod usage_stats;
#[cfg(target_os = "windows")]
mod app_score;
#[cfg(target_os = "windows")]
mod uninstall;
#[cfg(target_os = "windows")]
mod update;

#[cfg(target_os = "windows")]
use tauri::{
    CustomMenuItem, Manager, RunEvent, SystemTray, SystemTrayEvent, SystemTrayMenu,
    SystemTrayMenuItem,
};

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

/// 构建扫描进度发射器：节流约 200ms，最后一次（scanned >= total）必定发出。
#[cfg(target_os = "windows")]
fn progress_emitter(
    app: tauri::AppHandle,
    target: String,
    source: &'static str,
) -> impl FnMut(u64, Option<u64>) {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |scanned, total| {
        let finished = total.map(|t| scanned >= t).unwrap_or(false);
        if !finished && last.elapsed() < std::time::Duration::from_millis(200) {
            return;
        }
        last = std::time::Instant::now();
        let percent = total
            .filter(|t| *t > 0)
            .map(|t| (scanned as f64 / t as f64 * 100.0).clamp(0.0, 100.0));
        let _ = app.emit_all(
            "scan-progress",
            serde_json::json!({
                "target": target,
                "source": source,
                "scanned": scanned,
                "total": total,
                "percent": percent,
                "done": finished,
            }),
        );
    }
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn scan_directory(
    app: tauri::AppHandle,
    dir_path: String,
    top_n: usize,
) -> disk_usage::UsageReport {
    disk_usage::reset_cancel();
    tauri::async_runtime::spawn_blocking(move || {
        let mut emit = progress_emitter(app, dir_path.clone(), "walk");
        let report = disk_usage::scan_directory_with_progress(dir_path.clone(), top_n, &mut emit);
        log_startup(&format!(
            "scan_directory done: dir={} scanned={} dirs={} root={} source={}",
            dir_path, report.scanned, report.dirs.len(), report.root_size, report.source
        ));
        report
    })
    .await
    .unwrap_or_else(|e| {
        log_startup(&format!("scan_directory join error: {}", e));
        disk_usage::UsageReport {
            large_files: vec![],
            dirs: vec![],
            scanned: 0,
            errors: 0,
            source: "walk".into(),
            elapsed_ms: 0,
            cancelled: false,
            root_size: 0,
        }
    })
}

/// 把盘符规范化为卷根路径（保留冒号，去掉多余尾部反斜杠后补一个）。
/// "C:" / "C:\\" → "C:\\"；绝不能剥掉冒号，否则会拼出相对路径 "C\"。
#[cfg(target_os = "windows")]
fn volume_root(drive: &str) -> String {
    format!("{}\\", drive.trim().trim_end_matches('\\'))
}

/// 按盘符扫描整卷：优先走 MFT（需管理员、仅 NTFS），失败时回退到逐目录递归。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn scan_volume(
    app: tauri::AppHandle,
    drive: String,
    top_n: usize,
) -> disk_usage::UsageReport {
    disk_usage::reset_cancel();
    tauri::async_runtime::spawn_blocking(move || {
        let drive_for_walk = drive.clone();
        let mut emit = progress_emitter(app.clone(), drive.clone(), "mft");
        let report = match mft_scan::scan_volume_with_progress(&drive, top_n, &mut |done, total| {
            emit(done, Some(total))
        }) {
            Ok(report) => report,
            Err(_) => {
                // 回退：从盘根递归遍历（慢，但不依赖 MFT/权限）
                let root = volume_root(&drive_for_walk);
                let mut emit = progress_emitter(app, drive_for_walk, "walk");
                disk_usage::scan_directory_with_progress(root, top_n, &mut emit)
            }
        };
        log_startup(&format!(
            "scan_volume done: scanned={} dirs={} root={} errors={} source={}",
            report.scanned, report.dirs.len(), report.root_size, report.errors, report.source
        ));
        report
    })
    .await
    .unwrap_or_else(|e| {
        log_startup(&format!("scan_volume join error: {}", e));
        disk_usage::UsageReport {
            large_files: vec![],
            dirs: vec![],
            scanned: 0,
            errors: 0,
            source: "walk".into(),
            elapsed_ms: 0,
            cancelled: false,
            root_size: 0,
        }
    })
}

/// 列出一个目录的直接文件（TreeSize 式展开时按需加载文件行）。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn list_dir_files(dir_path: String, limit: usize) -> Vec<disk_usage::DirFileEntry> {
    tauri::async_runtime::spawn_blocking(move || disk_usage::list_dir_files(&dir_path, limit))
        .await
        .unwrap_or_default()
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
fn get_auto_boost() -> autoboost::AutoBoostState {
    autoboost::get_state()
}

/// 前端挂载完成后调用，用于诊断「WebView 已创建但页面未渲染」类白屏。
#[cfg(target_os = "windows")]
#[tauri::command]
fn log_ui_ready() {
    log_startup("ui ready: frontend mounted");
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn set_auto_boost(enabled: bool, threshold: u8) -> Result<autoboost::AutoBoostState, String> {
    autoboost::set_config(enabled, threshold)
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn list_startup() -> Vec<startup::StartupItem> {
    // 含 shell:AppsFolder 枚举（数百项），必须离开主线程
    tauri::async_runtime::spawn_blocking(startup::list_startup)
        .await
        .unwrap_or_default()
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

/// 读盘取"最近 `seconds` 秒"的历史，抽稀到至多 `max_points` 个点（按 `ts_ms` 升序）。
/// 走 `spawn_blocking`：读盘绝不允许阻塞 IPC/UI 线程。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn get_history_span(seconds: u64, max_points: usize) -> Vec<collector::MetricsSnapshot> {
    tauri::async_runtime::spawn_blocking(move || history_store::read_span(seconds, max_points))
        .await
        .unwrap_or_default()
}

/// 历史持久化状态（是否在写、目录、占用字节、最旧/最新时间、降级原因）。
#[cfg(target_os = "windows")]
#[tauri::command]
fn get_history_status() -> history_store::HistoryStatus {
    history_store::status()
}

/// 检查 GitHub 最新正式 release（只读公开元数据；不下载、不安装、不改本机）。
/// 网络请求放阻塞线程池，避免占住 IPC 线程导致界面卡顿。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn check_update(current_version: String) -> Result<update::UpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(move || update::check_update(&current_version))
        .await
        .unwrap_or_else(|_| Err("更新检查任务异常退出".to_string()))
}

/// 下载并安装更新：按当前安装形态（便携版 / NSIS / MSI）自动完成替换或安装，
/// 成功后进程会重启（本命令不会正常返回）。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    url: String,
    kind: String,
    digest: Option<String>,
    size: u64,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        update::install_update(&app, &url, &kind, digest.as_deref(), size)
    })
    .await
    .unwrap_or_else(|_| Err("更新任务异常退出".to_string()))
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
async fn run_health_check() -> Result<health_check::HealthReport, String> {
    tauri::async_runtime::spawn_blocking(health_check::run_health_check)
        .await
        .unwrap_or_else(|_| Err("体检任务异常退出".to_string()))
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
#[tauri::command]
async fn list_installed_apps() -> Vec<app_inventory::AppEntry> {
    tauri::async_runtime::spawn_blocking(app_inventory::list_installed_apps)
        .await
        .unwrap_or_default()
}

/// 批量提取应用图标（data URL）。失败项不返回，前端用首字母兜底。
#[cfg(target_os = "windows")]
#[tauri::command]
async fn get_app_icons(
    items: Vec<app_icon::IconRequest>,
) -> std::collections::HashMap<String, String> {
    tauri::async_runtime::spawn_blocking(move || app_icon::resolve_icons(items))
        .await
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn resolve_footprint(app_id: String) -> Result<footprint::Footprint, String> {
    tauri::async_runtime::spawn_blocking(move || footprint::resolve_by_id(&app_id))
        .await
        .unwrap_or_else(|_| Err("产物解析任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn list_apps_scored() -> Vec<app_score::ScoredApp> {
    tauri::async_runtime::spawn_blocking(app_score::list_scored)
        .await
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn uninstall_capability(app_id: String) -> Result<uninstall::UninstallCapability, String> {
    tauri::async_runtime::spawn_blocking(move || uninstall::uninstall_capability(&app_id))
        .await
        .unwrap_or_else(|_| Err("读取卸载能力任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn start_uninstaller(app_id: String, allow_quiet: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        uninstall::start_uninstaller(&app_id, allow_quiet)
    })
        .await
        .unwrap_or_else(|_| Err("启动卸载程序任务异常退出".to_string()))
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn is_app_installed(app_id: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || uninstall::is_app_installed(&app_id))
        .await
        .unwrap_or(true)
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn remove_residue(app_id: String, targets: Vec<String>) -> uninstall::RemovalResult {
    tauri::async_runtime::spawn_blocking(move || uninstall::remove_residue(&app_id, targets))
        .await
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
#[tauri::command]
async fn undo_removal(undo_token: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || uninstall::undo_removal(&undo_token))
        .await
        .unwrap_or_else(|_| Err("撤销任务异常退出".to_string()))
}

/// 托盘常驻：显示主窗口（从隐藏/最小化恢复并置前）。
#[cfg(target_os = "windows")]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// ---------------------------------------------------------------------------
// 单实例互斥体（句柄存全局，供「以管理员重启」交接时主动释放）
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
static INSTANCE_MUTEX: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(target_os = "windows")]
fn store_instance_mutex(handle: windows::Win32::Foundation::HANDLE) {
    INSTANCE_MUTEX.store(
        handle.0 as isize,
        std::sync::atomic::Ordering::SeqCst,
    );
}

/// 释放单实例互斥体。提权重启前必须调用：否则新提权实例会误判「已有实例」，
/// 找到旧窗口就自我退出（表现为「管理员重启没反应、应用直接没了」）。
#[cfg(target_os = "windows")]
pub(crate) fn release_single_instance() {
    let raw = INSTANCE_MUTEX.swap(0, std::sync::atomic::Ordering::SeqCst);
    if raw != 0 {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(
                raw as *mut core::ffi::c_void,
            ));
        }
    }
}

/// 重新获取单实例互斥体（提权重启被取消时的回滚）。
#[cfg(target_os = "windows")]
pub(crate) fn reacquire_single_instance() {
    use windows::core::w;
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        if let Ok(handle) = CreateMutexW(None, BOOL(1), w!("WinTop_SingleInstance")) {
            store_instance_mutex(handle);
        }
    }
}

/// 单实例守卫：托盘常驻后再次双击 exe 应唤起已有窗口，而不是开第二份。
/// 必须在便携版更新助手接管之后调用（助手进程也会进入 main，不能把它挡掉）。
#[cfg(target_os = "windows")]
fn ensure_single_instance() {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{GetLastError, BOOL, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_SHOW,
    };
    unsafe {
        // 句柄故意不由 Rust drop 处理：存入全局，进程存活期间一直持有；
        // 提权重启前由 release_single_instance() 主动关闭。
        match CreateMutexW(None, BOOL(1), w!("WinTop_SingleInstance")) {
            Ok(handle) => store_instance_mutex(handle),
            Err(_) => return, // 创建失败（极少见）时不做单实例限制
        }
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return;
        }
        // 已有实例：尝试把它的窗口唤到前台。
        // 首次冷启动的窗口可能要 1~2 秒才创建，因此最多等 3 秒；
        // 仍唤不起（旧实例卡死 / 处于不同提权级）就继续作为独立实例运行，
        // 避免「双击没反应」。
        for _ in 0..30 {
            if let Ok(hwnd) = FindWindowW(PCWSTR::null(), w!("Win-Top")) {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                if IsWindowVisible(hwnd).as_bool() {
                    std::process::exit(0);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// 启动过程日志（诊断白屏/卡死；`%LOCALAPPDATA%\WinTop\startup.log`，超过 256 KB 自动截断）。
#[cfg(target_os = "windows")]
fn log_startup(msg: &str) {
    use std::io::Write;
    let dir = crate::history_store::base_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("startup.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 256 * 1024 {
            let _ = std::fs::write(&path, b"");
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            "[{}] {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            msg
        );
    }
}

/// 主窗口的 WebView2 是否已就绪（自动加速等后台任务在此之前不应干扰启动）。
/// 主窗口由 tauri.conf.json 声明、在 setup 之前创建，因此 setup 开始时即已就绪。
#[cfg(target_os = "windows")]
static WEBVIEW_READY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub(crate) fn webview_ready() -> bool {
    WEBVIEW_READY.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(target_os = "windows")]
fn main() {
    log_startup(&format!(
        "main start: elevated={} args={:?}",
        privilege::is_elevated(),
        std::env::args().collect::<Vec<_>>()
    ));
    // 便携版自动更新：更新助手进程会在 Tauri 初始化前接管，替换 exe 后退出。
    // 必须在任何 Tauri/窗口初始化之前，也必须在单实例守卫之前。
    if let Some(code) = update::run_helper_if_requested() {
        std::process::exit(code);
    }
    ensure_single_instance();
    log_startup("single-instance check done");

    let tray = SystemTray::new()
        .with_tooltip("Win-Top：关闭窗口后在后台运行")
        .with_icon(tauri::Icon::Raw(include_bytes!("../icons/icon.png").to_vec()))
        .with_menu(
            SystemTrayMenu::new()
                .add_item(CustomMenuItem::new("show", "显示主界面"))
                .add_item(CustomMenuItem::new("boost", "立即加速"))
                .add_native_item(SystemTrayMenuItem::Separator)
                .add_item(CustomMenuItem::new("quit", "退出")),
        );

    let app = tauri::Builder::default()
        .system_tray(tray)
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "show" => show_main_window(app),
                "boost" => {
                    let handle = app.clone();
                    std::thread::spawn(move || {
                        let r = memboost::memory_boost();
                        let _ = handle.tray_handle().set_tooltip(&format!(
                            "Win-Top：手动加速完成，释放 {:.0} MB",
                            r.freed_mb
                        ));
                        let _ = handle.emit_all(
                            "boost-result",
                            serde_json::json!({
                                "source": "tray",
                                "freed_mb": r.freed_mb,
                                "trimmed_count": r.trimmed_count,
                            }),
                        );
                    });
                }
                "quit" => app.exit(0),
                _ => {}
            },
            SystemTrayEvent::LeftClick { .. } | SystemTrayEvent::DoubleClick { .. } => {
                show_main_window(app)
            }
            _ => {}
        })
        .setup(|app| {
            let handle = app.handle();
            // 主窗口由 tauri.conf.json 声明，在 setup 之前已创建完成（WebView2 已就绪）
            WEBVIEW_READY.store(true, std::sync::atomic::Ordering::SeqCst);
            log_startup("setup: start");
            // 上次便携版更新留下的 .old 备份（新进程退出前一直被占用）
            update::cleanup_old_binary();
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
            // 每进程 GPU 占用追踪（PDH GPU Engine 计数器，每 2 秒采集；纯推送，无新 command）
            gpu_proc::start(handle.clone());
            // 历史趋势持久化（定长追加 + 保留/淘汰；磁盘不可用时自动退回纯内存）
            history_store::start(handle.clone());
            // 自动加速：内存达阈值时自动裁剪工作集（设置持久化）
            autoboost::start(handle.clone());
            // 初始化定时清理调度器
            scheduler::init_scheduler(handle.clone());
            Ok(())
        })
        .on_window_event(|event| {
            // 关闭按钮 = 隐藏到托盘，不退出进程；真正退出走托盘菜单
            if let tauri::WindowEvent::CloseRequested { api, .. } = event.event() {
                api.prevent_close();
                let _ = event.window().hide();
            }
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
            list_dir_files,
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
            get_auto_boost,
            set_auto_boost,
            log_ui_ready,
            list_startup,
            set_startup_enabled,
            get_history,
            get_history_span,
            get_history_status,
            check_update,
            install_update,
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
            restore_visual_fx,
            list_installed_apps,
            get_app_icons,
            resolve_footprint,
            list_apps_scored,
            uninstall_capability,
            start_uninstaller,
            is_app_installed,
            remove_residue,
            undo_removal
        ])
        .build(tauri::generate_context!())
        .expect("error while building Win-Top");

    log_startup("app: entering event loop");
    app.run(|_app_handle, event| {
        if let RunEvent::Exit = event {
            // 兜底：无论从哪条路径退出（托盘退出/更新重启/系统注销），都让历史写线程落盘
            history_store::shutdown();
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Win-Top 仅支持 Windows 运行。");
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::volume_root;

    #[test]
    fn volume_root_keeps_drive_colon() {
        assert_eq!(volume_root("C:"), "C:\\");
        assert_eq!(volume_root("C:\\"), "C:\\");
        assert_eq!(volume_root(" j: "), "j:\\");
    }
}
