//! Windows 服务管理：通过 PowerShell Get-Service/Set-Service 查询和修改。
//! 使用 PS 而非 sc.exe 以正确解析中文显示名称（避免 GBK 乱码）。
//! 修改前自动保存原始状态，支持一键还原。

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::Mutex;
use std::sync::LazyLock;
use serde::Serialize;

const CREATE_NO_WINDOW: u32 = 0x08000000;

static ROLLBACK: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub status: String,
    pub startup_type: String,
    pub is_ms: bool,
}

#[derive(Serialize)]
pub struct ServiceActionResult {
    pub success: bool,
    pub message: String,
    pub can_rollback: bool,
}

/// 列出所有非微软的自动启动服务。
pub fn list_non_ms_services() -> Result<Vec<ServiceInfo>, String> {
    // 用 PowerShell Get-Service 输出 JSON，编码为 UTF-8
    let script = r#"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Get-Service | Where-Object {
    $_.StartType -eq 'Automatic' -and
    $_.ServiceName -notmatch '^(ms_|Microsoft|Win|Wdi|Wpn|Xbox|wl|WSearch|wscsvc|wuauserv|WaaS|Wallet|web|WFDS|WEP|Wer|werc|Wia|WinHttp|Wlan|wmi|wpc|WPD|wsc|WSMan|wudf|Wwan|AppMgmt|AppReadiness|AppXSvc|AssignedAccessManager|BFE|BITS|BrokerInfrastructure|BTAGService|BthAvctpSvc|bthserv|camsvc|CDPSvc|CDPUserSvc|ClipSVC|COMSysApp|CoreMessagingRegistrar|CryptSvc|DcomLaunch|defragsvc|DeviceInstall|Dhcp|DiagTrack|DispBrokerDesktopSvc|DmEnrollmentSvc|dmwappushsvc|Dnscache|DoSvc|DsSvc|DusmSvc|Eaphost|edgeupdate|EFS|EntAppSvc|EventLog|EventSystem|fdPHost|FDResPub|fhsvc|FontCache|FrameServer|gpsvc|hidserv|IKEEXT|iphlpsvc|KeyIso|KtmRm|LanmanServer|LanmanWorkstation|lfsvc|LicenseManager|lmhosts|LSM|MPSSVC|MSDTC|MSiSCSI|msiserver|NaturalAuthentication|NcaSvc|NcbService|NcdAutoSetup|Netlogon|Netman|NetMsmqActivator|NetPipeActivator|NetTcpActivator|NetTcpPortSharing|NlaSvc|nsi|p2pimsvc|PcaSvc|PeerDistSvc|PerfHost|pla|PlugPlay|PNRPAutoReg|PNRPsvc|PolicyAgent|Power|ProfSvc|PushToInstall|RasAuto|RasMan|RemoteAccess|RemoteRegistry|RmSvc|RpcEptMapper|RpcLocator|RpcSs|SamSs|SCardSvr|Schedule|SCPolicySvc|SDRSVC|seclogon|SENS|SensorDataService|SensorService|SensrSvc|SessionEnv|SharedAccess|ShellHWDetection|smphost|SNMPTRAP|Spooler|SSDPSRV|StiSvc|StorSvc|swprv|SysMain|SystemEventsBroker|TabletInputService|TapiSrv|TermService|Themes|TieringEngineService|TokenBroker|TrkWks|TroubleshootingSvc|TrustedInstaller|tzautoupdate|UdkUserSvc|UevAgentService|uhssvc|UmRdpService|upnphost|UserManager|UsoSvc|VaultSvc|vds|VSS|W32Time|W3SVC|WarpJITSvc|wbengine|WbioSrvc|Wcmsvc|wcncsvc|WdiServiceHost|WdiSystemHost|WebClient|Wecsvc|WEPHOSTSVC|wercplsupport|WerSvc|WiaRpc|WinHttpAutoProxySvc|Winmgmt|WinRM|wisvc|WlanSvc|wlpasvc|WManSvc|WMPNetworkSvc|workfolderssvc|WPDBusEnum|WpnService|wscsvc|WSearch|wuauserv|wudfsvc|WwanSvc)$'
} | Select-Object @{N='n';E={$_.ServiceName}},@{N='d';E={$_.DisplayName}},@{N='s';E={$_.Status.ToString()}},@{N='t';E={$_.StartType.ToString()}} | ConvertTo-Json -Compress
"#;

    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-Command", script])
        .output()
        .map_err(|e| format!("PowerShell 执行失败: {}", e))?;

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() || text == "[]" {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    struct Raw { n: Option<String>, d: Option<String>, s: Option<String>, t: Option<String> }

    let raw: Vec<Raw> = if text.starts_with('[') {
        serde_json::from_str(&text).unwrap_or_default()
    } else if text.starts_with('{') {
        serde_json::from_str::<Raw>(&text).ok().into_iter().collect()
    } else {
        return Ok(Vec::new());
    };

    let result: Vec<ServiceInfo> = raw.into_iter().map(|r| {
        let name = r.n.unwrap_or_default();
        ServiceInfo {
            is_ms: false,
            display_name: r.d.unwrap_or_else(|| name.clone()),
            name,
            status: r.s.unwrap_or_else(|| "Unknown".into()),
            startup_type: r.t.unwrap_or_else(|| "Automatic".into()),
        }
    }).collect();

    Ok(result)
}

/// 修改服务启动类型，自动保存原始值。
pub fn set_service_startup(name: &str, startup_type: &str) -> ServiceActionResult {
    let st = match startup_type {
        "auto" => "Automatic",
        "demand" => "Manual",
        "disabled" => "Disabled",
        _ => return ServiceActionResult { success: false, message: format!("无效类型: {}", startup_type), can_rollback: false },
    };

    // 保存原始值
    if let Ok(list) = list_non_ms_services() {
        if let Some(svc) = list.iter().find(|s| s.name == name) {
            if let Ok(mut rb) = ROLLBACK.lock() {
                if !rb.contains_key(name) {
                    rb.insert(name.to_string(), svc.startup_type.clone());
                }
            }
        }
    }

    let script = format!("Set-Service -Name '{}' -StartupType {} -ErrorAction Stop", name, st);
    let output = Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-Command", &script])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let label = match startup_type { "auto" => "自动", "demand" => "手动", "disabled" => "禁用", _ => startup_type };
            ServiceActionResult { success: true, message: format!("服务「{}」已设为「{}」", name, label), can_rollback: true }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            ServiceActionResult { success: false, message: format!("操作失败（可能需要管理员权限）: {}", err.trim()), can_rollback: false }
        }
        Err(e) => ServiceActionResult { success: false, message: format!("执行失败: {}", e), can_rollback: false },
    }
}

pub fn restore_service(name: &str) -> ServiceActionResult {
    let original = ROLLBACK.lock().ok().and_then(|rb| rb.get(name).cloned());
    match original {
        Some(orig) => {
            let map_back = |t: &str| match t {
                "Automatic" => "auto", "Manual" => "demand", "Disabled" => "disabled", _ => "demand",
            };
            let result = set_service_startup_force(name, map_back(&orig));
            if result.success {
                if let Ok(mut rb) = ROLLBACK.lock() { rb.remove(name); }
                ServiceActionResult { success: true, message: format!("服务「{}」已还原为「{}」", name, orig), can_rollback: false }
            } else { result }
        }
        None => ServiceActionResult { success: false, message: format!("没有「{}」的还原记录", name), can_rollback: false },
    }
}

pub fn list_modified_services() -> Vec<(String, String)> {
    ROLLBACK.lock().map(|rb| rb.iter().map(|(k,v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

pub fn restore_all_services() -> ServiceActionResult {
    let modified = list_modified_services();
    if modified.is_empty() {
        return ServiceActionResult { success: true, message: "没有需要还原的服务".into(), can_rollback: false };
    }
    let mut ok = 0; let mut fail = 0;
    for (name, _) in &modified {
        if restore_service(name).success { ok += 1; } else { fail += 1; }
    }
    ServiceActionResult { success: fail == 0, message: format!("已还原 {} 个服务{}", ok, if fail > 0 { format!("，{} 个失败", fail) } else { String::new() }), can_rollback: false }
}

fn set_service_startup_force(name: &str, sc_type: &str) -> ServiceActionResult {
    let st = match sc_type { "auto" => "Automatic", "demand" => "Manual", "disabled" => "Disabled", _ => return ServiceActionResult { success: false, message: String::new(), can_rollback: false } };
    let script = format!("Set-Service -Name '{}' -StartupType {} -ErrorAction SilentlyContinue", name, st);
    let output = Command::new("powershell").creation_flags(CREATE_NO_WINDOW).args(["-NoProfile", "-Command", &script]).output();
    match output {
        Ok(o) if o.status.success() => ServiceActionResult { success: true, message: String::new(), can_rollback: false },
        Ok(o) => ServiceActionResult { success: false, message: String::from_utf8_lossy(&o.stderr).trim().to_string(), can_rollback: false },
        Err(e) => ServiceActionResult { success: false, message: e.to_string(), can_rollback: false },
    }
}
