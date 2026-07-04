//! 文件解锁工具：查找占用指定文件的进程。
//! 使用 PowerShell 查询方式（通过进程模块匹配），尝试定位占用进程。

use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Serialize)]
pub struct FileLockInfo {
    pub pid: u32,
    pub process_name: String,
    pub file_path: String,
}

/// 查找占用指定文件的进程。
///
/// 注意：当前实现通过 PowerShell 匹配进程加载的模块文件名，仅能找到将目标文件
/// 作为可执行模块加载的进程。对于文件句柄锁定（如 Word 打开文档），结果可能为空。
/// 更精确的方案可使用 Sysinternals handle.exe 或 Restart Manager API（需管理员）。
pub fn find_locks(file_path: &str) -> Result<Vec<FileLockInfo>, String> {
    let script = format!(
        r#"$path = '{}'; Get-Process | Where-Object {{ $_.Modules.FileName -eq $path }} | Select-Object Id,ProcessName | ConvertTo-Json -Compress"#,
        file_path.replace('\'', "''")
    );
    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| format!("执行失败: {}", e))?;

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct PsResult {
        Id: Option<u32>,
        ProcessName: Option<String>,
    }

    let results: Vec<PsResult> = if text.starts_with('[') {
        serde_json::from_str(&text).unwrap_or_default()
    } else if text.starts_with('{') {
        serde_json::from_str::<PsResult>(&text)
            .ok()
            .into_iter()
            .collect()
    } else {
        return Ok(Vec::new());
    };

    Ok(results
        .into_iter()
        .map(|r| FileLockInfo {
            pid: r.Id.unwrap_or(0),
            process_name: r.ProcessName.unwrap_or_else(|| "未知".to_string()),
            file_path: file_path.to_string(),
        })
        .collect())
}
