//! 防火墙规则管理：通过 netsh advfirewall 查询/创建/删除规则。
//! 需要管理员权限。

use std::os::windows::process::CommandExt;
use std::process::Command;
use serde::Serialize;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Serialize)]
pub struct FirewallRule {
    pub name: String,
    pub enabled: bool,
    pub direction: String, // In/Out
    pub action: String,    // Allow/Block
    pub protocol: String,
    pub local_port: String,
    pub remote_ip: String,
}

pub fn list_rules() -> Result<Vec<FirewallRule>, String> {
    let output = Command::new("netsh")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "advfirewall",
            "firewall",
            "show",
            "rule",
            "name=all",
            "verbose",
        ])
        .output()
        .map_err(|e| format!("执行 netsh 失败: {}", e))?;

    let text = String::from_utf8_lossy(&output.stdout);
    parse_netsh_output(&text)
}

fn parse_netsh_output(text: &str) -> Result<Vec<FirewallRule>, String> {
    let mut rules = Vec::new();
    let mut current: Option<FirewallRule> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("规则名称:") || trimmed.starts_with("Rule Name:") {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            let name = trimmed
                .splitn(2, ':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_string();
            current = Some(FirewallRule {
                name,
                enabled: true,
                direction: "-".into(),
                action: "-".into(),
                protocol: "-".into(),
                local_port: "-".into(),
                remote_ip: "-".into(),
            });
        } else if let Some(ref mut rule) = current {
            if trimmed.starts_with("已启用:") || trimmed.starts_with("Enabled:") {
                rule.enabled =
                    trimmed.to_lowercase().contains("yes") || trimmed.contains("是");
            } else if trimmed.starts_with("方向:") || trimmed.starts_with("Direction:") {
                rule.direction = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .unwrap_or("-")
                    .trim()
                    .to_string();
            } else if trimmed.starts_with("操作:") || trimmed.starts_with("Action:") {
                rule.action = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .unwrap_or("-")
                    .trim()
                    .to_string();
            } else if trimmed.starts_with("协议:") || trimmed.starts_with("Protocol:") {
                rule.protocol = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .unwrap_or("-")
                    .trim()
                    .to_string();
            } else if trimmed.starts_with("本地端口:") || trimmed.starts_with("LocalPort:") {
                rule.local_port = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .unwrap_or("-")
                    .trim()
                    .to_string();
            } else if trimmed.starts_with("远程 IP:") || trimmed.starts_with("RemoteIP:") {
                rule.remote_ip = trimmed
                    .splitn(2, ':')
                    .nth(1)
                    .unwrap_or("-")
                    .trim()
                    .to_string();
            }
        }
    }
    if let Some(rule) = current.take() {
        rules.push(rule);
    }
    Ok(rules)
}

pub fn toggle_rule(name: &str, enabled: bool) -> Result<String, String> {
    let output = Command::new("netsh")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "advfirewall",
            "firewall",
            "set",
            "rule",
            &format!("name={}", name),
            "new",
            &format!(
                "enable={}",
                if enabled { "yes" } else { "no" }
            ),
        ])
        .output()
        .map_err(|e| format!("执行失败: {}", e))?;

    if output.status.success() {
        Ok(format!(
            "规则 '{}' 已{}",
            name,
            if enabled { "启用" } else { "禁用" }
        ))
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!("操作失败: {}", err))
    }
}
