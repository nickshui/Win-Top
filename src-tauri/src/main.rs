#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Local;
use serde::Serialize;
use std::sync::{LazyLock, Mutex};
use sysinfo::{Disks, Networks, System};

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
    pid: u32,
    name: String,
    cpu: f32,
    memory: String,
}

#[derive(Serialize)]
struct ProcessDetail {
    pid: u32,
    name: String,
    cpu: String,
    memory: String,
    path: String,
}

#[derive(Serialize)]
struct ActionResult {
    success: bool,
    message: String,
}

#[derive(Serialize, Clone)]
struct ActionLogEntry {
    timestamp: String,
    module: String,
    action: String,
    target: String,
    success: bool,
    message: String,
}

static ACTION_LOGS: LazyLock<Mutex<Vec<ActionLogEntry>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn append_action_log(module: &str, action: &str, target: &str, success: bool, message: &str) {
    let entry = ActionLogEntry {
        timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        module: module.to_string(),
        action: action.to_string(),
        target: target.to_string(),
        success,
        message: message.to_string(),
    };

    if let Ok(mut logs) = ACTION_LOGS.lock() {
        logs.push(entry);
        if logs.len() > 500 {
            let overflow = logs.len() - 500;
            logs.drain(0..overflow);
        }
    }
}

#[derive(Serialize)]
struct PortOverviewItem {
    port: u16,
    protocol: String,
    process: String,
    pid: u32,
}

#[derive(Serialize)]
struct ToolboxItem {
    id: String,
    name: String,
    description: String,
    command: String,
    requires_admin: bool,
    shell: String,
}

#[cfg_attr(target_os = "windows", tauri::command)]
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

    let disks = Disks::new_with_refreshed_list();
    let (total_disk, available_disk) = disks
        .iter()
        .filter(|disk| !disk.is_removable())
        .fold((0u64, 0u64), |acc, disk| {
            (acc.0 + disk.total_space(), acc.1 + disk.available_space())
        });
    let disk_usage = if total_disk > 0 {
        ((total_disk - available_disk) as f32 / total_disk as f32) * 100.0
    } else {
        0.0
    };

    let networks = Networks::new_with_refreshed_list();
    let total_network_bytes: u64 = networks
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

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_process_overview() -> Vec<ProcessOverviewItem> {
    let mut system = System::new_all();
    system.refresh_all();

    let mut processes: Vec<ProcessOverviewItem> = system
        .processes()
        .values()
        .map(|process| ProcessOverviewItem {
            pid: process.pid().as_u32(),
            name: process.name().to_string(),
            cpu: process.cpu_usage(),
            memory: format!("{:.1} MB", process.memory() as f32 / 1024.0),
        })
        .collect();

    processes.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
    processes.truncate(5);
    processes
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_process_detail(pid: u32) -> Option<ProcessDetail> {
    let mut system = System::new_all();
    system.refresh_all();

    system.processes().get(&sysinfo::Pid::from_u32(pid)).map(|process| {
        ProcessDetail {
            pid,
            name: process.name().to_string(),
            cpu: format!("{:.0}%", process.cpu_usage()),
            memory: format!("{:.1} MB", process.memory() as f32 / 1024.0),
            path: process
                .exe()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| "-".to_string()),
        }
    })
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn terminate_process(pid: u32) -> ActionResult {
    let mut system = System::new_all();
    system.refresh_all();

    let result = match system.processes().get(&sysinfo::Pid::from_u32(pid)) {
        Some(process) => {
            if process.kill() {
                ActionResult {
                    success: true,
                    message: format!("进程已结束（PID {}）。", pid),
                }
            } else {
                ActionResult {
                    success: false,
                    message: "结束进程失败：可能需要管理员权限。".to_string(),
                }
            }
        }
        None => ActionResult {
            success: false,
            message: "未找到进程或已退出。".to_string(),
        },
    };

    append_action_log(
        "process",
        "terminate_process",
        &format!("pid:{}", pid),
        result.success,
        &result.message,
    );

    result
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn set_process_priority(_pid: u32, _level: String) -> ActionResult {
    let result = ActionResult {
        success: false,
        message: "设置优先级暂未实现，需要管理员权限策略与平台适配。".to_string(),
    };

    append_action_log(
        "process",
        "set_process_priority",
        &format!("pid:{}", _pid),
        result.success,
        &result.message,
    );

    result
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_port_overview() -> Vec<PortOverviewItem> {
    vec![
        PortOverviewItem {
            port: 3000,
            protocol: "TCP".to_string(),
            process: "Node".to_string(),
            pid: 2316,
        },
        PortOverviewItem {
            port: 5432,
            protocol: "TCP".to_string(),
            process: "PostgreSQL".to_string(),
            pid: 412,
        },
        PortOverviewItem {
            port: 6379,
            protocol: "TCP".to_string(),
            process: "Redis".to_string(),
            pid: 902,
        },
    ]
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_toolbox_items() -> Vec<ToolboxItem> {
    vec![
        ToolboxItem {
            id: "net-diagnose".to_string(),
            name: "网络诊断".to_string(),
            description: "执行基础网络诊断与修复命令。".to_string(),
            command: "ipconfig /flushdns".to_string(),
            requires_admin: true,
            shell: "cmd".to_string(),
        },
        ToolboxItem {
            id: "disk-clean".to_string(),
            name: "磁盘清理".to_string(),
            description: "清理临时文件并释放空间。".to_string(),
            command: "cleanmgr".to_string(),
            requires_admin: false,
            shell: "cmd".to_string(),
        },
        ToolboxItem {
            id: "system-repair".to_string(),
            name: "系统修复".to_string(),
            description: "扫描并修复系统文件。".to_string(),
            command: "sfc /scannow".to_string(),
            requires_admin: true,
            shell: "cmd".to_string(),
        },
        ToolboxItem {
            id: "free-port".to_string(),
            name: "释放端口".to_string(),
            description: "查找并释放占用端口的进程。".to_string(),
            command: "netstat -ano".to_string(),
            requires_admin: false,
            shell: "powershell".to_string(),
        },
    ]
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn run_toolbox_command(id: String) -> ActionResult {
    let tools = get_toolbox_items();
    let tool = match tools.into_iter().find(|item| item.id == id) {
        Some(tool) => tool,
        None => {
            let result = ActionResult {
                success: false,
                message: "未找到该命令。".to_string(),
            };
            append_action_log("toolbox", "run_toolbox_command", &id, result.success, &result.message);
            return result;
        }
    };

    if tool.requires_admin {
        let result = ActionResult {
            success: false,
            message: "该命令需要管理员权限，请以管理员身份运行 Win-Top。".to_string(),
        };
        append_action_log(
            "toolbox",
            "run_toolbox_command",
            &tool.id,
            result.success,
            &result.message,
        );
        return result;
    }

    let output = match tool.shell.as_str() {
        "powershell" => std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &tool.command])
            .output(),
        _ => std::process::Command::new("cmd")
            .args(["/C", &tool.command])
            .output(),
    };

    let result = match output {
        Ok(result) => {
            if result.status.success() {
                let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
                ActionResult {
                    success: true,
                    message: format!(
                        "执行成功（{}）：{}{}",
                        tool.shell,
                        tool.command,
                        format_output(&stdout)
                    ),
                }
            } else {
                let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
                ActionResult {
                    success: false,
                    message: format!(
                        "执行失败（{}）：{}{}",
                        tool.shell,
                        tool.command,
                        format_output(&stderr)
                    ),
                }
            }
        }
        Err(error) => ActionResult {
            success: false,
            message: format!("执行失败：无法启动命令（{}）。", error),
        },
    };

    append_action_log(
        "toolbox",
        "run_toolbox_command",
        &tool.id,
        result.success,
        &result.message,
    );

    result
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_action_logs() -> Vec<ActionLogEntry> {
    ACTION_LOGS
        .lock()
        .map(|logs| logs.clone())
        .unwrap_or_else(|_| Vec::new())
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn export_action_logs(format: String) -> ActionResult {
    let logs = ACTION_LOGS
        .lock()
        .map(|logs| logs.clone())
        .unwrap_or_else(|_| Vec::new());

    if logs.is_empty() {
        return ActionResult {
            success: false,
            message: "暂无可导出的日志。".to_string(),
        };
    }

    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let path = match format.as_str() {
        "csv" => std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(format!("win_top_logs_{}.csv", timestamp)),
        _ => std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(format!("win_top_logs_{}.json", timestamp)),
    };

    let write_result = if format == "csv" {
        let mut output = String::from("timestamp,module,action,target,success,message\n");
        for item in &logs {
            let escaped = item.message.replace('"', "''").replace(',', "，");
            output.push_str(&format!(
                "{},{},{},{},{},\"{}\"\n",
                item.timestamp, item.module, item.action, item.target, item.success, escaped
            ));
        }
        std::fs::write(&path, output)
    } else {
        serde_json::to_string_pretty(&logs)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .and_then(|body| std::fs::write(&path, body))
    };

    match write_result {
        Ok(_) => ActionResult {
            success: true,
            message: format!("日志已导出到 {}", path.display()),
        },
        Err(error) => ActionResult {
            success: false,
            message: format!("日志导出失败：{}", error),
        },
    }
}

fn format_output(output: &str) -> String {
    if output.is_empty() {
        "".to_string()
    } else {
        format!("\n{}", output)
    }
}

#[cfg(target_os = "windows")]
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_monitor_snapshot,
            get_process_overview,
            get_process_detail,
            terminate_process,
            set_process_priority,
            get_port_overview,
            get_toolbox_items,
            run_toolbox_command,
            get_action_logs,
            export_action_logs
        ])
        .run(tauri::generate_context!())
        .expect("error while running Win-Top");
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Win-Top 仅支持 Windows 运行。当前环境可用于代码检查。");
}
