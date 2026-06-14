//! Kernel-Network ETW POC：确认 per-进程网络事件的 event_id 与字段名。
//! 需以管理员运行。采集 ~12 秒，统计各 event_id 计数，并对样本事件
//! 尝试多个候选字段名提取 PID / 字节数，结果写入 nettraffic-poc.txt。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::UserTrace;
use ferrisetw::EventRecord;

const KERNEL_NETWORK_GUID: &str = "7DD42A49-5329-4832-8DFD-43D979153A88";

static COUNTS: LazyLock<Mutex<HashMap<u16, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SAMPLES: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn probe(record: &EventRecord, locator: &SchemaLocator) {
    let schema = match locator.event_schema(record) {
        Ok(s) => s,
        Err(_) => return,
    };
    let id = record.event_id();
    {
        let mut c = COUNTS.lock().unwrap();
        *c.entry(id).or_insert(0) += 1;
    }
    let mut samples = SAMPLES.lock().unwrap();
    if samples.len() < 40 {
        let parser = Parser::create(record, &schema);
        let pid = ["PID", "Pid", "ProcessId", "ProcessID"]
            .iter()
            .find_map(|k| parser.try_parse::<u32>(k).ok().map(|v| format!("{}={}", k, v)))
            .unwrap_or_else(|| "PID=<none>".into());
        let size = ["size", "Size", "Length", "NumBytes"]
            .iter()
            .find_map(|k| parser.try_parse::<u32>(k).ok().map(|v| format!("{}={}", k, v)))
            .unwrap_or_else(|| "size=<none>".into());
        samples.push(format!("id={:<3} {} {}", id, pid, size));
    }
}

fn main() {
    let provider = Provider::by_guid(KERNEL_NETWORK_GUID)
        .add_callback(probe)
        .build();

    let trace = UserTrace::new()
        .named("WinTopNetPoc".to_string())
        .enable(provider)
        .start_and_process();

    let mut out = String::new();
    match trace {
        Ok(_t) => {
            println!("ETW 会话已启动，采集 12 秒（请同时下载文件/看视频制造流量）…");
            std::thread::sleep(Duration::from_secs(12));
            out.push_str("=== event_id 计数 ===\n");
            let counts = COUNTS.lock().unwrap();
            let mut ids: Vec<_> = counts.iter().collect();
            ids.sort_by_key(|(k, _)| **k);
            for (id, n) in ids {
                out.push_str(&format!("id={:<3} count={}\n", id, n));
            }
            out.push_str("\n=== 样本字段探测 ===\n");
            for s in SAMPLES.lock().unwrap().iter() {
                out.push_str(s);
                out.push('\n');
            }
        }
        Err(e) => {
            out.push_str(&format!("ETW 会话启动失败（多半未提权）：{:?}\n", e));
        }
    }
    let _ = std::fs::write("nettraffic-poc.txt", &out);
    println!("{out}");
}
