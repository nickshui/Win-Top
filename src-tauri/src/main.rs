#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Local;
use serde::Serialize;
use std::collections::HashMap;
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
struct DiskItem {
    name: String,
    mount_point: String,
    file_system: String,
    total: String,
    available: String,
    used_percent: f32,
    removable: bool,
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_disk_overview() -> Vec<DiskItem> {
    let disks = Disks::new_with_refreshed_list();
    let mut items: Vec<DiskItem> = disks
        .iter()
        .map(|disk| {
            let total = disk.total_space();
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let used_percent = if total > 0 {
                (used as f32 / total as f32) * 100.0
            } else {
                0.0
            };
            DiskItem {
                name: disk.name().to_string_lossy().to_string(),
                mount_point: disk.mount_point().to_string_lossy().to_string(),
                file_system: disk.file_system().to_string_lossy().to_string(),
                total: format_bytes(total),
                available: format_bytes(available),
                used_percent,
                removable: disk.is_removable(),
            }
        })
        .collect();
    items.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    items
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

fn map_priority_level(level: &str) -> Option<&'static str> {
    match level {
        "低" | "Idle" | "idle" => Some("Idle"),
        "低于普通" | "BelowNormal" => Some("BelowNormal"),
        "普通" | "Normal" | "normal" => Some("Normal"),
        "高于普通" | "AboveNormal" => Some("AboveNormal"),
        "高" | "High" | "high" => Some("High"),
        "实时" | "RealTime" | "realtime" => Some("RealTime"),
        _ => None,
    }
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn set_process_priority(pid: u32, level: String) -> ActionResult {
    let priority_class = match map_priority_level(&level) {
        Some(p) => p,
        None => {
            let result = ActionResult {
                success: false,
                message: format!("无效的优先级：{}", level),
            };
            append_action_log(
                "process",
                "set_process_priority",
                &format!("pid:{} level:{}", pid, level),
                result.success,
                &result.message,
            );
            return result;
        }
    };

    let script = format!(
        "try {{ $p = Get-Process -Id {} -ErrorAction Stop; $p.PriorityClass = '{}'; Write-Output 'OK' }} catch {{ Write-Error $_.Exception.Message; exit 1 }}",
        pid, priority_class
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output();

    let result = match output {
        Ok(out) if out.status.success() => ActionResult {
            success: true,
            message: format!("优先级已设置为 {}（PID {}）。", level, pid),
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let hint = if priority_class == "RealTime" {
                "（实时优先级通常需要管理员权限）"
            } else {
                "（可能需要管理员权限或进程不存在）"
            };
            ActionResult {
                success: false,
                message: format!("设置优先级失败{}：{}", hint, stderr),
            }
        }
        Err(e) => ActionResult {
            success: false,
            message: format!("无法调用 PowerShell：{}", e),
        },
    };

    append_action_log(
        "process",
        "set_process_priority",
        &format!("pid:{} level:{}", pid, level),
        result.success,
        &result.message,
    );

    result
}

#[derive(Serialize)]
struct PortOverviewResult {
    items: Vec<PortOverviewItem>,
    total: usize,
    error: Option<String>,
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn get_port_overview() -> PortOverviewResult {
    match enumerate_listening_ports() {
        Ok(items) => {
            let total = items.len();
            PortOverviewResult {
                items,
                total,
                error: None,
            }
        }
        Err(e) => PortOverviewResult {
            items: Vec::new(),
            total: 0,
            error: Some(format!("枚举端口失败：{}", e)),
        },
    }
}

fn enumerate_listening_ports() -> Result<Vec<PortOverviewItem>, String> {
    let output = std::process::Command::new("netstat")
        .args(["-ano"])
        .output()
        .map_err(|e| format!("无法执行 netstat: {}", e))?;

    if !output.status.success() {
        return Err("netstat 命令执行失败".to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Build PID -> process name map
    let mut system = System::new_all();
    system.refresh_all();
    let pid_name_map: HashMap<u32, String> = system
        .processes()
        .iter()
        .map(|(pid, proc_info)| (pid.as_u32(), proc_info.name().to_string()))
        .collect();

    let mut items: Vec<PortOverviewItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in stdout.lines() {
        let line = line.trim();

        // Match lines like:
        //   TCP    0.0.0.0:3000    0.0.0.0:0    LISTENING    1234
        //   TCP    [::]:3000       [::]:0       LISTENING    1234
        //   UDP    0.0.0.0:5353   *:*                        1234
        let parts: Vec<&str> = line.split_whitespace().collect();

        let (protocol, local_addr, pid_str) = if parts.len() >= 5
            && (parts[0] == "TCP" || parts[0] == "tcp")
            && parts[3].eq_ignore_ascii_case("LISTENING")
        {
            (parts[0], parts[1], parts[4])
        } else if parts.len() >= 4 && (parts[0] == "UDP" || parts[0] == "udp") {
            // UDP has no state column
            (parts[0], parts[1], parts[3])
        } else {
            continue;
        };

        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Extract port from address like "0.0.0.0:3000" or "[::]:3000"
        let port: u16 = match local_addr.rsplit(':').next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };

        // Deduplicate: same port+protocol (IPv4 and IPv6 both listen)
        let key = (port, protocol.to_uppercase());
        if !seen.insert(key) {
            continue;
        }

        let process_name = pid_name_map
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| if pid == 0 { "System Idle".to_string() } else { format!("PID {}", pid) });

        items.push(PortOverviewItem {
            port,
            protocol: protocol.to_uppercase(),
            process: process_name,
            pid,
        });
    }

    items.sort_by_key(|item| item.port);
    Ok(items)
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

#[derive(Serialize, Clone)]
struct AiScriptStep {
    rationale: String,
    command: String,
    shell: String,
}

#[derive(Serialize, Clone)]
struct AiScriptRecipe {
    id: String,
    name: String,
    description: String,
    risk: String,
    requires_admin: bool,
    steps: Vec<AiScriptStep>,
}

fn build_recipes() -> Vec<AiScriptRecipe> {
    vec![
        AiScriptRecipe {
            id: "memory-audit".to_string(),
            name: "内存占用审计".to_string(),
            description: "列出占用内存最高的前 10 个进程，便于定位内存压力来源。".to_string(),
            risk: "low".to_string(),
            requires_admin: false,
            steps: vec![AiScriptStep {
                rationale: "检查前 10 个高内存进程".to_string(),
                command: "Get-Process | Sort-Object -Property WorkingSet64 -Descending | Select-Object -First 10 Name, Id, @{Name='Memory(MB)';Expression={[math]::Round($_.WorkingSet64/1MB,1)}} | Format-Table -AutoSize".to_string(),
                shell: "powershell".to_string(),
            }],
        },
        AiScriptRecipe {
            id: "disk-cleanup".to_string(),
            name: "磁盘清理建议".to_string(),
            description: "查询磁盘空间并检查当前用户 Temp 目录占用，供用户手动确认后清理。".to_string(),
            risk: "medium".to_string(),
            requires_admin: false,
            steps: vec![
                AiScriptStep {
                    rationale: "列出各分区剩余空间".to_string(),
                    command: "Get-PSDrive -PSProvider FileSystem | Format-Table Name, @{N='Used(GB)';E={[math]::Round(($_.Used/1GB),1)}}, @{N='Free(GB)';E={[math]::Round(($_.Free/1GB),1)}} -AutoSize".to_string(),
                    shell: "powershell".to_string(),
                },
                AiScriptStep {
                    rationale: "统计当前用户 Temp 目录大小（不删除）".to_string(),
                    command: "Get-ChildItem -Path $env:TEMP -Recurse -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum | Select-Object @{N='TempSize(MB)';E={[math]::Round($_.Sum/1MB,1)}}".to_string(),
                    shell: "powershell".to_string(),
                },
            ],
        },
        AiScriptRecipe {
            id: "daily-healthcheck".to_string(),
            name: "日常健康检查".to_string(),
            description: "执行基础网络与系统状态检查。".to_string(),
            risk: "low".to_string(),
            requires_admin: false,
            steps: vec![
                AiScriptStep {
                    rationale: "刷新 DNS 缓存以排除解析异常".to_string(),
                    command: "ipconfig /flushdns".to_string(),
                    shell: "cmd".to_string(),
                },
                AiScriptStep {
                    rationale: "查看当前监听端口总数".to_string(),
                    command: "netstat -ano | findstr LISTENING | find /c \"LISTENING\"".to_string(),
                    shell: "cmd".to_string(),
                },
            ],
        },
    ]
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn generate_ai_script() -> AiScriptRecipe {
    let mut system = System::new_all();
    system.refresh_all();
    let total_memory = system.total_memory() as f32;
    let used_memory = system.used_memory() as f32;
    let mem_pct = if total_memory > 0.0 { used_memory / total_memory * 100.0 } else { 0.0 };

    let disks = Disks::new_with_refreshed_list();
    let (total_disk, available_disk) = disks
        .iter()
        .filter(|d| !d.is_removable())
        .fold((0u64, 0u64), |acc, d| (acc.0 + d.total_space(), acc.1 + d.available_space()));
    let disk_pct = if total_disk > 0 {
        (total_disk - available_disk) as f32 / total_disk as f32 * 100.0
    } else {
        0.0
    };

    let recipes = build_recipes();
    let id = if mem_pct >= 70.0 {
        "memory-audit"
    } else if disk_pct >= 80.0 {
        "disk-cleanup"
    } else {
        "daily-healthcheck"
    };
    recipes.into_iter().find(|r| r.id == id).unwrap()
}

#[cfg_attr(target_os = "windows", tauri::command)]
fn run_ai_script(id: String) -> ActionResult {
    let recipes = build_recipes();
    let recipe = match recipes.into_iter().find(|r| r.id == id) {
        Some(r) => r,
        None => {
            let result = ActionResult {
                success: false,
                message: format!("未找到脚本配方：{}", id),
            };
            append_action_log("ai-script", "run_ai_script", &id, false, &result.message);
            return result;
        }
    };

    if recipe.requires_admin {
        let result = ActionResult {
            success: false,
            message: "该脚本需要管理员权限。".to_string(),
        };
        append_action_log("ai-script", "run_ai_script", &recipe.id, false, &result.message);
        return result;
    }

    let mut combined = String::new();
    let mut all_success = true;
    for step in &recipe.steps {
        let output = match step.shell.as_str() {
            "powershell" => std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &step.command])
                .output(),
            _ => std::process::Command::new("cmd")
                .args(["/C", &step.command])
                .output(),
        };
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                combined.push_str(&format!("▶ {}\n", step.rationale));
                if !stdout.trim().is_empty() {
                    combined.push_str(stdout.trim());
                    combined.push('\n');
                }
                if !stderr.trim().is_empty() {
                    combined.push_str(&format!("[stderr] {}\n", stderr.trim()));
                }
                if !out.status.success() {
                    all_success = false;
                }
            }
            Err(e) => {
                all_success = false;
                combined.push_str(&format!("▶ {}\n执行失败：{}\n", step.rationale, e));
            }
        }
    }

    let result = ActionResult {
        success: all_success,
        message: format!("【{}】\n{}", recipe.name, combined.trim_end()),
    };
    append_action_log("ai-script", "run_ai_script", &recipe.id, result.success, &recipe.name);
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
            get_disk_overview,
            get_toolbox_items,
            run_toolbox_command,
            generate_ai_script,
            run_ai_script,
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
