//! 网络与端口模块：用 IP Helper API（GetExtendedTcpTable/UdpTable，经 netstat2 封装）
//! 结构化枚举 TCP/UDP 连接 -> PID -> 进程名。不再 spawn netstat、无 GBK 乱码。

use serde::Serialize;

use netstat2::{
    get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
};

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
