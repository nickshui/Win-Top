//! 磁盘温度探针 v3：用带 __Path 的具名 struct 取物理盘路径，
//! 再 ASSOCIATORS OF 关联到 MSFT_StorageReliabilityCounter 拿温度。
//! 需以管理员运行。结果写入 disktemp-result.txt。

use std::collections::HashMap;

use serde::Deserialize;
use wmi::{COMLibrary, Variant, WMIConnection};

#[derive(Deserialize)]
#[serde(rename = "MSFT_PhysicalDisk")]
#[allow(non_snake_case)]
struct PhysDisk {
    DeviceId: Option<String>,
    FriendlyName: Option<String>,
    __Path: Option<String>,
}

fn main() {
    let out = run().unwrap_or_else(|e| format!("ERROR: {e}"));
    let _ = std::fs::write("disktemp-result.txt", &out);
    println!("{out}");
}

fn run() -> Result<String, String> {
    let com = COMLibrary::new().map_err(|e| e.to_string())?;
    let con = WMIConnection::with_namespace_path("ROOT\\Microsoft\\Windows\\Storage", com)
        .map_err(|e| e.to_string())?;
    let mut s = String::new();

    let disks: Vec<PhysDisk> = con.query().map_err(|e| e.to_string())?;
    s.push_str(&format!("=== MSFT_PhysicalDisk ({} rows) ===\n", disks.len()));

    for d in &disks {
        s.push_str(&format!(
            "DeviceId={:?} Friendly={:?}\n  __Path={:?}\n",
            d.DeviceId, d.FriendlyName, d.__Path
        ));
        if let Some(path) = &d.__Path {
            let q = format!(
                "ASSOCIATORS OF {{{}}} WHERE ResultClass = MSFT_StorageReliabilityCounter",
                path
            );
            match con.raw_query::<HashMap<String, Variant>>(&q) {
                Ok(rows) => {
                    s.push_str(&format!("  assoc rows: {}\n", rows.len()));
                    for r in &rows {
                        s.push_str(&format!(
                            "    Temperature={:?}  TemperatureMax={:?}  Wear={:?}\n",
                            r.get("Temperature"),
                            r.get("TemperatureMax"),
                            r.get("Wear"),
                        ));
                    }
                }
                Err(e) => s.push_str(&format!("  assoc ERROR: {e}\n")),
            }
        }
    }

    Ok(s)
}
