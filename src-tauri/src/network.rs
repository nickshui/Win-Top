//! 网络与端口模块：用 IP Helper API（GetExtendedTcpTable/UdpTable，经 netstat2 封装）
//! 结构化枚举 TCP/UDP 连接 -> PID -> 进程名。不再 spawn netstat、无 GBK 乱码。

use std::ffi::c_void;
use std::mem::size_of;
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::time::Instant;

use serde::Serialize;

use netstat2::{
    get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
};

use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, ICMP_ECHO_REPLY, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR, SOCKADDR_IN};

#[derive(Serialize)]
pub struct PortRow {
    pub port: u16,
    pub protocol: String,
    pub family: String, // IPv4 / IPv6
    pub state: String,
    pub local_addr: String,
    pub remote: String,
    pub pid: u32,
    pub process: String,
}

fn tcp_state_label(state: &TcpState) -> &'static str {
    match state {
        TcpState::Listen => "LISTEN",
        TcpState::Established => "ESTABLISHED",
        TcpState::SynSent => "SYN_SENT",
        TcpState::SynReceived => "SYN_RECV",
        TcpState::FinWait1 => "FIN_WAIT1",
        TcpState::FinWait2 => "FIN_WAIT2",
        TcpState::CloseWait => "CLOSE_WAIT",
        TcpState::Closing => "CLOSING",
        TcpState::LastAck => "LAST_ACK",
        TcpState::TimeWait => "TIME_WAIT",
        TcpState::Closed => "CLOSED",
        TcpState::DeleteTcb => "DELETE_TCB",
        _ => "UNKNOWN",
    }
}

pub fn list_connections() -> Result<Vec<PortRow>, String> {
    let pid_names = crate::process::pid_name_map();

    let af_flags = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let proto_flags = ProtocolFlags::TCP | ProtocolFlags::UDP;
    let sockets =
        get_sockets_info(af_flags, proto_flags).map_err(|e| format!("枚举端口失败：{}", e))?;

    let resolve = |pid: u32| -> String {
        pid_names.get(&pid).cloned().unwrap_or_else(|| {
            if pid == 0 {
                "System Idle".to_string()
            } else {
                format!("PID {}", pid)
            }
        })
    };

    let mut rows = Vec::new();
    for si in sockets {
        let pid = si.associated_pids.first().copied().unwrap_or(0);
        let process = resolve(pid);
        match si.protocol_socket_info {
            ProtocolSocketInfo::Tcp(tcp) => rows.push(PortRow {
                port: tcp.local_port,
                protocol: "TCP".to_string(),
                family: if tcp.local_addr.is_ipv6() { "IPv6" } else { "IPv4" }.to_string(),
                state: tcp_state_label(&tcp.state).to_string(),
                local_addr: tcp.local_addr.to_string(),
                remote: if tcp.state == TcpState::Listen {
                    "*".to_string()
                } else {
                    format!("{}:{}", tcp.remote_addr, tcp.remote_port)
                },
                pid,
                process,
            }),
            ProtocolSocketInfo::Udp(udp) => rows.push(PortRow {
                port: udp.local_port,
                protocol: "UDP".to_string(),
                family: if udp.local_addr.is_ipv6() { "IPv6" } else { "IPv4" }.to_string(),
                state: "-".to_string(),
                local_addr: udp.local_addr.to_string(),
                remote: "*".to_string(),
                pid,
                process,
            }),
        }
    }

    rows.sort_by(|a, b| a.port.cmp(&b.port).then(a.protocol.cmp(&b.protocol)));
    Ok(rows)
}

// ===== 网络一键体检 =====

#[derive(Serialize)]
pub struct AdapterInfo {
    pub name: String,
    pub ips: Vec<String>,
    pub gateway: Option<String>,
}

#[derive(Serialize)]
pub struct PingResult {
    pub label: String,
    pub target: String,
    pub ok: bool,
    pub latency_ms: Option<u32>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct NetCheckup {
    pub adapters: Vec<AdapterInfo>,
    pub public_ip: Option<String>,
    pub dns_ms: Option<u64>,
    pub pings: Vec<PingResult>,
}

unsafe fn sockaddr_ipv4(sa: *const SOCKADDR) -> Option<String> {
    if sa.is_null() {
        return None;
    }
    if (*sa).sa_family == AF_INET {
        let sin = &*(sa as *const SOCKADDR_IN);
        let o = sin.sin_addr.S_un.S_addr.to_ne_bytes();
        Some(format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]))
    } else {
        None
    }
}

fn list_adapters() -> Vec<AdapterInfo> {
    let mut out = Vec::new();
    unsafe {
        let family = AF_INET.0 as u32;
        let flags = GAA_FLAG_INCLUDE_GATEWAYS
            | GAA_FLAG_SKIP_ANYCAST
            | GAA_FLAG_SKIP_MULTICAST
            | GAA_FLAG_SKIP_DNS_SERVER;

        let mut size = 0u32;
        // 第一次取所需缓冲大小
        GetAdaptersAddresses(family, flags, None, None, &mut size);
        if size == 0 {
            return out;
        }
        let mut buf = vec![0u8; size as usize];
        let ret = GetAdaptersAddresses(
            family,
            flags,
            None,
            Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
            &mut size,
        );
        if ret != 0 {
            return out;
        }

        let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !cur.is_null() {
            let a = &*cur;
            // OperStatus == IfOperStatusUp(1)
            if a.OperStatus.0 == 1 {
                let name = a.FriendlyName.to_string().unwrap_or_default();
                let mut ips = Vec::new();
                let mut ua = a.FirstUnicastAddress;
                while !ua.is_null() {
                    if let Some(ip) = sockaddr_ipv4((*ua).Address.lpSockaddr) {
                        ips.push(ip);
                    }
                    ua = (*ua).Next;
                }
                let mut gateway = None;
                let mut ga = a.FirstGatewayAddress;
                while !ga.is_null() {
                    if let Some(ip) = sockaddr_ipv4((*ga).Address.lpSockaddr) {
                        gateway = Some(ip);
                        break;
                    }
                    ga = (*ga).Next;
                }
                if !ips.is_empty() {
                    out.push(AdapterInfo { name, ips, gateway });
                }
            }
            cur = a.Next;
        }
    }
    out
}

fn ping(label: &str, ip: Ipv4Addr) -> PingResult {
    let target = ip.to_string();
    unsafe {
        let handle = match IcmpCreateFile() {
            Ok(h) => h,
            Err(e) => {
                return PingResult {
                    label: label.to_string(),
                    target,
                    ok: false,
                    latency_ms: None,
                    error: Some(format!("IcmpCreateFile 失败: {}", e.message())),
                }
            }
        };
        let dest = u32::from_ne_bytes(ip.octets());
        let send_data = [0u8; 32];
        let reply_size = (size_of::<ICMP_ECHO_REPLY>() + 32 + 8) as u32;
        let mut reply = vec![0u8; reply_size as usize];
        let n = IcmpSendEcho(
            handle,
            dest,
            send_data.as_ptr() as *const c_void,
            32,
            None,
            reply.as_mut_ptr() as *mut c_void,
            reply_size,
            1000,
        );
        let _ = IcmpCloseHandle(handle);

        if n > 0 {
            let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
            if r.Status == 0 {
                PingResult {
                    label: label.to_string(),
                    target,
                    ok: true,
                    latency_ms: Some(r.RoundTripTime),
                    error: None,
                }
            } else {
                PingResult {
                    label: label.to_string(),
                    target,
                    ok: false,
                    latency_ms: None,
                    error: Some(format!("无响应 (status {})", r.Status)),
                }
            }
        } else {
            PingResult {
                label: label.to_string(),
                target,
                ok: false,
                latency_ms: None,
                error: Some("超时 / 不可达".to_string()),
            }
        }
    }
}

// 裸 TCP HTTP 查询公网 IP（不引 reqwest/TLS）
fn public_ip() -> Option<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = ("api.ipify.org", 80).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: api.ipify.org\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok()?;
    resp.split("\r\n\r\n")
        .nth(1)
        .map(|b| b.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 64)
}

fn dns_ms() -> Option<u64> {
    let start = Instant::now();
    let ok = ("www.microsoft.com", 443)
        .to_socket_addrs()
        .map(|mut it| it.next().is_some())
        .unwrap_or(false);
    if ok {
        Some(start.elapsed().as_millis() as u64)
    } else {
        None
    }
}

pub fn checkup() -> NetCheckup {
    let adapters = list_adapters();

    // 目标：网关 + 国内(阿里/114/腾讯) + 海外(Google/Cloudflare)
    let mut targets: Vec<(String, Ipv4Addr)> = Vec::new();
    let mut seen_gw = std::collections::HashSet::new();
    for gw in adapters.iter().filter_map(|a| a.gateway.clone()) {
        if seen_gw.insert(gw.clone()) {
            if let Ok(ip) = gw.parse::<Ipv4Addr>() {
                targets.push(("网关".to_string(), ip));
            }
        }
    }
    targets.push(("阿里 DNS".to_string(), Ipv4Addr::new(223, 5, 5, 5)));
    targets.push(("114 DNS".to_string(), Ipv4Addr::new(114, 114, 114, 114)));
    targets.push(("腾讯 DNS".to_string(), Ipv4Addr::new(119, 29, 29, 29)));
    targets.push(("Google".to_string(), Ipv4Addr::new(8, 8, 8, 8)));
    targets.push(("Cloudflare".to_string(), Ipv4Addr::new(1, 1, 1, 1)));

    // 并行 ping（每个独立 ICMP 句柄，线程安全），总耗时≈单次超时
    let handles: Vec<_> = targets
        .into_iter()
        .map(|(label, ip)| std::thread::spawn(move || ping(&label, ip)))
        .collect();
    let pings: Vec<PingResult> = handles.into_iter().filter_map(|h| h.join().ok()).collect();

    // 公网 IP 与 DNS 解析并行
    let pub_handle = std::thread::spawn(public_ip);
    let dns_handle = std::thread::spawn(dns_ms);

    NetCheckup {
        adapters,
        public_ip: pub_handle.join().unwrap_or(None),
        dns_ms: dns_handle.join().unwrap_or(None),
        pings,
    }
}

// ===== 下行测速：限时下载测吞吐 =====

#[derive(Serialize)]
pub struct SpeedResult {
    pub down_mbps: f64,
    pub bytes: u64,
    pub secs: f64,
    pub streams: usize,
    pub error: Option<String>,
}

pub fn speed_test() -> SpeedResult {
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let fail = |msg: String| SpeedResult {
        down_mbps: 0.0,
        bytes: 0,
        secs: 0.0,
        streams: 0,
        error: Some(msg),
    };

    // 每流独立 Client（禁用 HTTP/2、连接池空闲连接置 0）→ N 个流 = N 条真实 TCP 连接，
    // 避免 h2 多路复用把多流合并回单连接、受单连接拥塞窗口限制而测不满带宽。
    let build_client = || {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(30))
            .http1_only()
            .pool_max_idle_per_host(0)
            .build()
    };

    // 主力源：Cloudflare speed 端点，全球 Anycast 就近、公开无鉴权、基本不限速，
    // 最能反映真实线路带宽。bytes 参数请求大对象以持续下载。
    // 补充源：国内镜像大文件作兜底，防止 Cloudflare 偶发不可达时无数据。
    let cf = "https://speed.cloudflare.com/__down?bytes=1000000000";
    let sources = [
        cf,
        cf,
        cf,
        "https://mirrors.aliyun.com/ubuntu-releases/22.04/ubuntu-22.04.5-desktop-amd64.iso",
        "https://mirrors.cloud.tencent.com/nodejs-release/v20.11.0/node-v20.11.0-x64.msi",
    ];
    let streams_per_source = 4;

    let total = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();

    for url in sources {
        for _ in 0..streams_per_source {
            let u = url.to_string();
            let t = Arc::clone(&total);
            let s = Arc::clone(&stop);
            handles.push(std::thread::spawn(move || {
                let client = match build_client() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let mut buf = [0u8; 65536];
                // 持续下载直到主线程置停；文件下完立即重新请求，保持并行度
                while !s.load(Ordering::Relaxed) {
                    match client.get(&u).send() {
                        Ok(mut resp) => {
                            if !resp.status().is_success() {
                                break;
                            }
                            loop {
                                match resp.read(&mut buf) {
                                    Ok(0) => break, // 文件读完，重新请求
                                    Ok(n) => {
                                        t.fetch_add(n as u64, Ordering::Relaxed);
                                        if s.load(Ordering::Relaxed) {
                                            return;
                                        }
                                    }
                                    Err(_) => return,
                                }
                            }
                        }
                        Err(_) => break, // 该源不可达，放弃
                    }
                }
            }));
        }
    }
    let stream_count = handles.len();

    // 自适应窗口：先等速率爬升到稳定（相邻采样斜率收敛）再开始计窗，
    // 摊薄 TCP 慢启动；随后测量直到速率方差收敛或达到最长时限 → 高带宽下也测得准。
    let sample = || total.load(Ordering::Relaxed);
    let tick = Duration::from_millis(500);

    // 阶段一：预热直到速率稳定（最多 6s）。以每 tick 增量近似瞬时速率，
    // 连续两次相对变化 < 12% 视为进入稳态。
    let warmup_deadline = Instant::now() + Duration::from_secs(6);
    let mut prev = sample();
    let mut prev_rate = 0.0f64;
    while Instant::now() < warmup_deadline {
        std::thread::sleep(tick);
        let now = sample();
        let rate = (now - prev) as f64 / tick.as_secs_f64();
        prev = now;
        if prev_rate > 0.0 {
            let delta = (rate - prev_rate).abs() / prev_rate.max(1.0);
            if delta < 0.12 {
                break; // 速率趋稳，结束预热
            }
        }
        prev_rate = rate;
    }

    // 阶段二：稳态测量窗口。最短 6s、最长 14s；每秒计算窗口内平均速率，
    // 当最近速率相对整窗均值的偏差 < 8% 时判定收敛、提前结束。
    let b0 = sample();
    let win_start = Instant::now();
    let min_win = Duration::from_secs(6);
    let max_win = Duration::from_secs(14);
    let mut last_avg = 0.0f64;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let elapsed = win_start.elapsed();
        let cur = sample();
        let avg = (cur - b0) as f64 / elapsed.as_secs_f64();
        if elapsed >= min_win {
            if elapsed >= max_win {
                break;
            }
            if last_avg > 0.0 {
                let drift = (avg - last_avg).abs() / last_avg.max(1.0);
                if drift < 0.08 {
                    break; // 平均速率收敛
                }
            }
        }
        last_avg = avg;
    }
    let b1 = sample();
    let secs = win_start.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    let bytes = b1.saturating_sub(b0);
    if bytes < 1_000_000 {
        return fail("测速失败：未下载到足够数据".to_string());
    }
    SpeedResult {
        down_mbps: (bytes as f64 * 8.0) / 1_000_000.0 / secs,
        bytes,
        secs,
        streams: stream_count,
        error: None,
    }
}

// ===== 自定义目标检测：DNS 解析 + ping + TCP 端口连通 =====

#[derive(Serialize)]
pub struct TcpProbe {
    pub port: u16,
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct TargetProbe {
    pub input: String,
    pub host: String,
    pub resolved: Vec<String>,
    pub ping: Option<PingResult>,
    pub tcp: Option<TcpProbe>,
    pub error: Option<String>,
}

pub fn probe_target(input: &str) -> TargetProbe {
    let input = input.trim().to_string();

    // 解析 host[:port]（IPv6 含冒号，这里仅对末段是合法端口时拆分）
    let (host, port) = match input.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), Some(port)),
            Err(_) => (input.clone(), None),
        },
        _ => (input.clone(), None),
    };

    if host.is_empty() {
        return TargetProbe {
            input,
            host,
            resolved: Vec::new(),
            ping: None,
            tcp: None,
            error: Some("请输入域名或 IP".to_string()),
        };
    }

    // DNS 解析
    let mut resolved: Vec<String> = Vec::new();
    if let Ok(addrs) = format!("{}:0", host).to_socket_addrs() {
        for a in addrs {
            let ip = a.ip().to_string();
            if !resolved.contains(&ip) {
                resolved.push(ip);
            }
        }
    }
    if resolved.is_empty() {
        return TargetProbe {
            input,
            host,
            resolved,
            ping: None,
            tcp: None,
            error: Some("无法解析该主机".to_string()),
        };
    }

    // ping 首个 IPv4
    let ping_res = resolved
        .iter()
        .find_map(|s| s.parse::<Ipv4Addr>().ok())
        .map(|ip| ping("目标", ip));

    // 指定端口则做 TCP 连通测试
    let tcp = port.map(|p| {
        use std::net::TcpStream;
        use std::time::Duration;
        let start = Instant::now();
        match format!("{}:{}", host, p)
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
        {
            Some(addr) => match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                Ok(_) => TcpProbe {
                    port: p,
                    ok: true,
                    latency_ms: Some(start.elapsed().as_millis() as u64),
                    error: None,
                },
                Err(e) => TcpProbe {
                    port: p,
                    ok: false,
                    latency_ms: None,
                    error: Some(e.to_string()),
                },
            },
            None => TcpProbe {
                port: p,
                ok: false,
                latency_ms: None,
                error: Some("地址解析失败".to_string()),
            },
        }
    });

    TargetProbe {
        input,
        host,
        resolved,
        ping: ping_res,
        tcp,
        error: None,
    }
}
