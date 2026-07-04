//! 导出系统状态快照为 JSON 格式。

use std::fs;
use std::path::PathBuf;
use serde::Serialize;

#[derive(Serialize)]
pub struct ExportResult {
    pub path: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// 生成系统快照 JSON 并写入文件。
/// 数据包括：进程列表、端口连接、磁盘报告、启动项。
pub fn export_snapshot() -> Result<ExportResult, String> {
    // Collect all data
    let processes = crate::process::list_processes().unwrap_or_default();
    let connections = crate::network::list_connections().unwrap_or_default();
    let disk = crate::disk::report();
    let startup = crate::startup::list_startup();
    
    #[derive(Serialize)]
    struct Snapshot {
        timestamp: String,
        processes: Vec<crate::process::ProcessRow>,
        connections: Vec<crate::network::PortRow>,
        disk: crate::disk::DiskReport,
        startup: Vec<crate::startup::StartupItem>,
    }
    
    let snap = Snapshot {
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        processes,
        connections,
        disk,
        startup,
    };
    
    // Write to desktop
    let desktop = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|p| p.join("Desktop"))
        .unwrap_or_else(|| PathBuf::from("."));
    
    let filename = format!("WinTop_Snapshot_{}.json", 
        chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let path = desktop.join(&filename);
    
    let json = serde_json::to_string_pretty(&snap)
        .map_err(|e| format!("JSON 序列化失败: {}", e))?;
    
    fs::write(&path, json)
        .map_err(|e| format!("写入文件失败: {}", e))?;
    
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        ok: true,
        error: None,
    })
}

/// 导出进程列表为 CSV
pub fn export_processes_csv() -> Result<ExportResult, String> {
    let processes = crate::process::list_processes().unwrap_or_default();
    
    let mut csv = String::from("PID,Name,CPU%,Memory_MB,Threads\n");
    for p in &processes {
        csv.push_str(&format!("{},{},{:.1},{:.1},{}\n", p.pid, p.name, p.cpu, p.mem_mb, p.threads));
    }
    
    let desktop = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|p| p.join("Desktop"))
        .unwrap_or_else(|| PathBuf::from("."));
    
    let filename = format!("WinTop_Processes_{}.csv",
        chrono::Local::now().format("%Y%m%d_%H%M%S"));
    let path = desktop.join(&filename);
    
    fs::write(&path, csv)
        .map_err(|e| format!("写入文件失败: {}", e))?;
    
    Ok(ExportResult {
        path: path.to_string_lossy().to_string(),
        ok: true,
        error: None,
    })
}
