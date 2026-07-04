//! 系统还原点管理：通过 PowerShell 查询/创建还原点。
//! 创建和删除需要管理员权限。

use std::os::windows::process::CommandExt;
use std::process::Command;
use serde::Serialize;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Serialize)]
pub struct RestorePoint {
    pub sequence: u32,
    pub description: String,
    pub creation_time: String,
    pub restore_point_type: String,
}

pub fn list_restore_points() -> Result<Vec<RestorePoint>, String> {
    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-NoProfile",
            "-Command",
            "Get-ComputerRestorePoint | Select-Object SequenceNumber,Description,CreationTime,RestorePointType | ConvertTo-Json -Compress",
        ])
        .output()
        .map_err(|e| format!("PowerShell 执行失败: {}", e))?;

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct Raw {
        SequenceNumber: Option<u32>,
        Description: Option<String>,
        CreationTime: Option<String>,
        RestorePointType: Option<u32>,
    }

    let raw: Vec<Raw> = if text.starts_with('[') {
        serde_json::from_str(&text).unwrap_or_default()
    } else if text.starts_with('{') {
        // Single object
        serde_json::from_str::<Raw>(&text)
            .ok()
            .into_iter()
            .collect()
    } else {
        return Ok(Vec::new());
    };

    let points: Vec<RestorePoint> = raw
        .into_iter()
        .map(|r| {
            let rt = match r.RestorePointType.unwrap_or(0) {
                0 => "应用安装",
                1 => "应用卸载",
                10 => "设备驱动安装",
                12 => "修改设置",
                13 => "取消操作",
                _ => "未知",
            };
            RestorePoint {
                sequence: r.SequenceNumber.unwrap_or(0),
                description: r.Description.unwrap_or_default(),
                creation_time: r.CreationTime.unwrap_or_default(),
                restore_point_type: rt.to_string(),
            }
        })
        .collect();

    Ok(points)
}

pub fn create_restore_point(description: &str) -> Result<String, String> {
    let script = format!(
        "Checkpoint-Computer -Description '{}' -RestorePointType APPLICATION_INSTALL",
        description.replace('\'', "''")
    );
    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| format!("执行失败: {}", e))?;

    if output.status.success() {
        Ok(format!("已创建还原点: {}", description))
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!("创建还原点失败: {}", err))
    }
}
