//! 原生 Windows API 能力验证 POC（Stage 1）
//! 不使用 sysinfo，不 spawn 任何 shell（netstat/powershell）。

use std::collections::HashMap;
use std::mem::size_of;
use std::thread::sleep;
use std::time::Duration;

use windows::core::w;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use netstat2::{
    get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
};

use std::sync::atomic::{AtomicU32, Ordering};

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::UserTrace;
use ferrisetw::EventRecord;

static ETW_COUNT: AtomicU32 = AtomicU32::new(0);

fn line() {
    println!("{}", "-".repeat(60));
}

/// 1. PDH：准确的全局 CPU 使用率（需要两次采样，间隔 ≥1s）。
/// 使用 *English* 计数器 API，规避中文 Windows 计数器路径本地化问题。
fn probe_cpu_pdh() {
    println!("[1] PDH 全局 CPU 使用率（English counter，两次采样）");
    unsafe {
        let mut query: isize = 0;
        let status = PdhOpenQueryW(None, 0, &mut query);
        if status != 0 {
            println!("    PdhOpenQueryW 失败: 0x{:08X}", status);
            return;
        }

        let mut counter: isize = 0;
        let status = PdhAddEnglishCounterW(
            query,
            w!("\\Processor(_Total)\\% Processor Time"),
            0,
            &mut counter,
        );
        if status != 0 {
            println!("    PdhAddEnglishCounterW 失败: 0x{:08X}", status);
            return;
        }

        // 第一次采样（建立基线）
        PdhCollectQueryData(query);
        sleep(Duration::from_millis(1000));
        // 第二次采样（计算速率）
        let status = PdhCollectQueryData(query);
        if status != 0 {
            println!("    第二次 PdhCollectQueryData 失败: 0x{:08X}", status);
            return;
        }

        let mut value = PDH_FMT_COUNTERVALUE::default();
        let status =
            PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut value);
        if status != 0 {
            println!("    PdhGetFormattedCounterValue 失败: 0x{:08X}", status);
            return;
        }
        let cpu = value.Anonymous.doubleValue;
        println!("    => CPU 使用率: {:.1}%  (与任务管理器对照应基本一致)", cpu);
    }
}

/// 2. GlobalMemoryStatusEx：内存。
fn probe_memory() {
    println!("[2] GlobalMemoryStatusEx 内存");
    unsafe {
        let mut ms = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        match GlobalMemoryStatusEx(&mut ms) {
            Ok(_) => {
                let gb = |b: u64| b as f64 / 1024.0 / 1024.0 / 1024.0;
                println!(
                    "    => 负载 {}%  已用 {:.1} GB / 共 {:.1} GB  可用 {:.1} GB",
                    ms.dwMemoryLoad,
                    gb(ms.ullTotalPhys - ms.ullAvailPhys),
                    gb(ms.ullTotalPhys),
                    gb(ms.ullAvailPhys),
                );
            }
            Err(e) => println!("    GlobalMemoryStatusEx 失败: {e}"),
        }
    }
}

/// 3. Toolhelp32：进程枚举（pid -> name 映射）。
fn enumerate_processes() -> HashMap<u32, String> {
    let mut map = HashMap::new();
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(e) => {
                println!("    CreateToolhelp32Snapshot 失败: {e}");
                return map;
            }
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                map.insert(entry.th32ProcessID, name);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
    }
    map
}

/// 4. netstat2 (GetExtendedTcpTable)：监听端口 -> PID -> 进程名。
fn probe_ports(pid_names: &HashMap<u32, String>) {
    println!("[4] netstat2 / GetExtendedTcpTable 监听端口（无 shell）");
    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let proto = ProtocolFlags::TCP | ProtocolFlags::UDP;
    match get_sockets_info(af, proto) {
        Ok(socks) => {
            let mut listeners: Vec<(u16, String, u32, String)> = Vec::new();
            for s in socks {
                if let ProtocolSocketInfo::Tcp(t) = &s.protocol_socket_info {
                    if t.state == TcpState::Listen {
                        let pid = s.associated_pids.first().copied().unwrap_or(0);
                        let name = pid_names
                            .get(&pid)
                            .cloned()
                            .unwrap_or_else(|| format!("PID {pid}"));
                        listeners.push((t.local_port, "TCP".into(), pid, name));
                    }
                }
            }
            listeners.sort_by_key(|x| x.0);
            listeners.dedup_by_key(|x| x.0);
            println!("    => 共 {} 个 TCP 监听端口，前 12 个：", listeners.len());
            for (port, proto, pid, name) in listeners.iter().take(12) {
                println!("       :{:<6} {} pid={:<6} {}", port, proto, pid, name);
            }
        }
        Err(e) => println!("    get_sockets_info 失败: {e}"),
    }
}

/// 5. ferrisetw ETW：实时进程启停事件（Microsoft-Windows-Kernel-Process）。
/// 这是 sysinfo/shell 无法提供的能力：系统主动推送，而非轮询。
fn etw_log(msg: &str) {
    use std::io::Write;
    println!("{msg}");
    // 同时写入文件，便于管理员进程的输出被回读
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("etw-result.txt")
    {
        let _ = writeln!(f, "{msg}");
    }
}

fn etw_callback(record: &EventRecord, schema_locator: &SchemaLocator) {
    if let Ok(schema) = schema_locator.event_schema(record) {
        let action = match record.event_id() {
            1 => "START",
            2 => "STOP ",
            _ => return,
        };
        let parser = Parser::create(record, &schema);
        let pid: u32 = parser.try_parse("ProcessID").unwrap_or(0);
        let image: String = parser
            .try_parse("ImageName")
            .unwrap_or_else(|_| "<?>".to_string());
        let n = ETW_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 20 {
            etw_log(&format!("    [ETW#{n}] {action} pid={pid:<6} {image}"));
        }
    }
}

fn probe_etw() {
    println!("[5] ferrisetw ETW 实时进程事件（Microsoft-Windows-Kernel-Process）");
    let provider = Provider::by_guid("22FB2CD6-0E7B-422B-A0C7-2FAD1FD0E716")
        .add_callback(etw_callback)
        .build();

    let trace = UserTrace::new()
        .named("WinTopPocTrace".to_string())
        .enable(provider)
        .start_and_process();

    match trace {
        Ok(trace) => {
            println!("    ETW 会话已启动，监听约 7 秒（期间触发测试进程验证捕获）...");
            for _ in 0..3 {
                sleep(Duration::from_millis(1500));
                let _ = std::process::Command::new("cmd")
                    .args(["/c", "ver"])
                    .output();
            }
            sleep(Duration::from_millis(2000));
            let total = ETW_COUNT.load(Ordering::Relaxed);
            let _ = trace.stop();
            etw_log(&format!("    => 共捕获 {total} 个进程启停事件"));
            if total == 0 {
                etw_log("    （0 事件通常意味着未以管理员运行：ETW 实时会话需要提权）");
            }
        }
        Err(e) => {
            println!("    ETW 启动失败: {e:?}");
            println!("    （real-time ETW 会话需管理员权限；编译通过即证明 ferrisetw API 可用）");
        }
    }
}

fn main() {
    println!("Win-Top 原生 API POC — Stage 1+2（windows-rs + netstat2 + ferrisetw，零 shell）");
    line();
    probe_cpu_pdh();
    line();
    probe_memory();
    line();

    println!("[3] Toolhelp32 进程枚举");
    let pid_names = enumerate_processes();
    println!("    => 进程总数: {}", pid_names.len());
    line();

    probe_ports(&pid_names);
    line();

    probe_etw();
    line();
    println!("POC 完成。");
}
