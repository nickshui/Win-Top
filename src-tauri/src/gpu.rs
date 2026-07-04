//! GPU 监控：通过 WMI 获取 GPU 基本信息，通过 PDH 获取利用率（如果可用）。
//! 支持多 GPU。非管理员也可读取基本 WMI 信息。

use serde::Serialize;

#[derive(Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: String,
    pub vram_mb: u64,
    pub utilization_pct: f64, // from PDH if available, else 0
}

/// Get GPU list with basic info via WMI.
pub fn list_gpus() -> Result<Vec<GpuInfo>, String> {
    // 在全新线程里执行，避免与 Tauri/WebView2 已初始化的 STA 套间冲突
    // （否则 COMLibrary::new() 会因套间模型不一致报 RPC_E_CHANGED_MODE 0x80010106）
    std::thread::spawn(|| -> Result<Vec<GpuInfo>, String> {
        let com = wmi::COMLibrary::new().map_err(|e| e.to_string())?;
        let con = wmi::WMIConnection::new(com).map_err(|e| e.to_string())?;

        #[derive(serde::Deserialize)]
        #[serde(rename = "Win32_VideoController")]
        #[serde(rename_all = "PascalCase")]
        struct WmiGpu {
            name: Option<String>,
            driver_version: Option<String>,
            adapter_ram: Option<u64>,
        }

        let results: Vec<WmiGpu> = con.query().map_err(|e| e.to_string())?;

        let gpus: Vec<GpuInfo> = results
            .into_iter()
            .map(|g| GpuInfo {
                name: g.name.unwrap_or_default().trim().to_string(),
                driver_version: g.driver_version.unwrap_or_default(),
                vram_mb: g.adapter_ram.unwrap_or(0) / 1024 / 1024,
                utilization_pct: 0.0,
            })
            .collect();

        // TODO: 后续可通过 PDH 计数器 "\GPU Engine(*engtype_3D)\Utilization Percentage"
        // 获取实时利用率，但需要 GPU 性能计数器支持（非所有系统可用）。
        Ok(gpus)
    })
    .join()
    .map_err(|_| "WMI GPU 查询线程异常退出".to_string())?
}
