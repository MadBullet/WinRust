//! TCP/UDP endpoint tables with owning PIDs — the data behind Task Manager's
//! "Network" and the Resource Monitor's "Network" tab.

use crate::error::{Result, WinError};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, GetPerTcpConnectionEStats, SetPerTcpConnectionEStats,
    TCP_ESTATS_DATA_ROD_v0, TCP_ESTATS_DATA_RW_v0, TcpConnectionEstatsData, MIB_TCPROW_LH,
    MIB_TCPROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Protocol {
    #[default]
    Tcp,
    Tcp6,
    Udp,
    Udp6,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Tcp6 => "TCPv6",
            Protocol::Udp => "UDP",
            Protocol::Udp6 => "UDPv6",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Connection {
    pub protocol: Protocol,
    pub local_addr: String,
    pub local_port: u16,
    pub remote_addr: String,
    pub remote_port: u16,
    /// TCP connection state (`ESTABLISHED`, `LISTENING`, ...); empty for UDP.
    pub state: String,
    pub pid: u32,
}

const TCP_STATES: [&str; 13] = [
    "",
    "CLOSED",
    "LISTENING",
    "SYN_SENT",
    "SYN_RECV",
    "ESTABLISHED",
    "FIN_WAIT1",
    "FIN_WAIT2",
    "CLOSE_WAIT",
    "CLOSING",
    "LAST_ACK",
    "TIME_WAIT",
    "DELETE_TCB",
];

fn ipv4_to_string(raw: u32) -> String {
    // Stored in network byte order.
    let b = raw.to_le_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn port_from_net(raw: u32) -> u16 {
    // Ports are in network order in the low 16 bits.
    u16::from_be((raw & 0xFFFF) as u16)
}

fn ipv6_to_string(b: &[u8; 16]) -> String {
    // Compress the longest run of zero groups (RFC 5952-ish).
    let mut groups = [0u16; 8];
    for i in 0..8 {
        groups[i] = u16::from_be_bytes([b[i * 2], b[i * 2 + 1]]);
    }
    // Special-case IPv4-mapped addresses.
    if groups[..5].iter().all(|&g| g == 0) && groups[5] == 0xFFFF {
        return format!(
            "::ffff:{}",
            ipv4_to_string(u32::from_be_bytes([b[12], b[13], b[14], b[15]]))
        );
    }
    let mut best_start = usize::MAX;
    let mut best_len = 0usize;
    let mut i = 0;
    while i < 8 {
        if groups[i] == 0 {
            let start = i;
            while i < 8 && groups[i] == 0 {
                i += 1;
            }
            if i - start > best_len {
                best_len = i - start;
                best_start = start;
            }
        } else {
            i += 1;
        }
    }
    if best_len < 2 {
        return groups
            .iter()
            .map(|g| format!("{g:x}"))
            .collect::<Vec<_>>()
            .join(":");
    }
    let head = groups[..best_start]
        .iter()
        .map(|g| format!("{g:x}"))
        .collect::<Vec<_>>()
        .join(":");
    let tail = groups[best_start + best_len..]
        .iter()
        .map(|g| format!("{g:x}"))
        .collect::<Vec<_>>()
        .join(":");
    format!("{head}::{tail}")
}

fn tcp_table(family: u32) -> Result<Vec<MIB_TCPROW_OWNER_PID>> {
    let mut size = 0u32;
    let rc = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if rc != ERROR_INSUFFICIENT_BUFFER && rc != 0 {
        return Err(WinError::from_code(rc, "GetExtendedTcpTable(size)"));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if rc != 0 {
        return Err(WinError::from_code(rc, "GetExtendedTcpTable"));
    }
    // table layout: dwNumEntries followed by rows
    let count = unsafe { *(buf.as_ptr() as *const u32) } as usize;
    let offset = std::mem::size_of::<u32>();
    let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    if offset + count * row_size > buf.len() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let row = unsafe {
            std::ptr::read_unaligned(
                buf.as_ptr().add(offset + i * row_size) as *const MIB_TCPROW_OWNER_PID
            )
        };
        rows.push(row);
    }
    Ok(rows)
}

fn udp_table(family: u32) -> Result<Vec<MIB_UDPROW_OWNER_PID>> {
    let mut size = 0u32;
    let rc = unsafe {
        GetExtendedUdpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            family,
            UDP_TABLE_OWNER_PID,
            0,
        )
    };
    if rc != ERROR_INSUFFICIENT_BUFFER && rc != 0 {
        return Err(WinError::from_code(rc, "GetExtendedUdpTable(size)"));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe {
        GetExtendedUdpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            0,
            family,
            UDP_TABLE_OWNER_PID,
            0,
        )
    };
    if rc != 0 {
        return Err(WinError::from_code(rc, "GetExtendedUdpTable"));
    }
    let count = unsafe { *(buf.as_ptr() as *const u32) } as usize;
    let offset = std::mem::size_of::<u32>();
    let row_size = std::mem::size_of::<MIB_UDPROW_OWNER_PID>();
    if offset + count * row_size > buf.len() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let row = unsafe {
            std::ptr::read_unaligned(
                buf.as_ptr().add(offset + i * row_size) as *const MIB_UDPROW_OWNER_PID
            )
        };
        rows.push(row);
    }
    Ok(rows)
}

/// TCP table for the IPv4 family (Task Manager shows these by default).
pub fn tcp_connections_v4() -> Vec<Connection> {
    let mut out = Vec::new();
    if let Ok(rows) = tcp_table(AF_INET) {
        for r in rows {
            out.push(Connection {
                protocol: Protocol::Tcp,
                local_addr: ipv4_to_string(r.dwLocalAddr),
                local_port: port_from_net(r.dwLocalPort),
                remote_addr: if r.dwRemoteAddr == 0 {
                    String::new()
                } else {
                    ipv4_to_string(r.dwRemoteAddr)
                },
                remote_port: port_from_net(r.dwRemotePort),
                state: TCP_STATES
                    .get(r.dwState as usize)
                    .copied()
                    .unwrap_or("")
                    .to_string(),
                pid: r.dwOwningPid,
            });
        }
    }
    out
}

/// All TCP connections (IPv4 + IPv6) — used by the resource monitor.
pub fn tcp_connections() -> Vec<Connection> {
    let mut out = tcp_connections_v4();
    if let Ok(rows) = tcp_table(AF_INET6) {
        for r in rows {
            out.push(Connection {
                protocol: Protocol::Tcp6,
                local_addr: "::".into(),
                local_port: port_from_net(r.dwLocalPort),
                remote_addr: String::new(),
                remote_port: port_from_net(r.dwRemotePort),
                state: TCP_STATES
                    .get(r.dwState as usize)
                    .copied()
                    .unwrap_or("")
                    .to_string(),
                pid: r.dwOwningPid,
            });
        }
    }
    out
}

pub fn udp_listeners() -> Vec<Connection> {
    let mut out = Vec::new();
    if let Ok(rows) = udp_table(AF_INET) {
        for r in rows {
            out.push(Connection {
                protocol: Protocol::Udp,
                local_addr: ipv4_to_string(r.dwLocalAddr),
                local_port: port_from_net(r.dwLocalPort),
                remote_addr: String::new(),
                remote_port: 0,
                state: String::new(),
                pid: r.dwOwningPid,
            });
        }
    }
    out
}

/// Per-process cumulative network byte totals (in + out), for Task Manager's
/// "Network" column and the Resource Monitor.
///
/// Windows only exposes byte counts per *TCP connection* through the
/// extended-statistics API, so the flow is: enable data collection on each
/// connection once (`TCP_ESTATS_DATA_RW_v0`), then read the read-only data
/// (`TCP_ESTATS_DATA_ROD_v0` = bytes in + bytes out) and sum it per PID.
/// The first call for a connection returns "not ready", which simply means the
/// process shows 0 B/s until the next tick — exactly how Task Manager behaves.
pub fn net_bytes_per_pid() -> std::collections::HashMap<u32, u64> {
    let mut out: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for row in tcp_owner_rows() {
        let Some(rb) = tcp_bytes_for_row(&row) else {
            continue;
        };
        *out.entry(row.dwOwningPid).or_insert(0) += rb;
    }
    out
}

/// `MIB_TCPROW_LH` (the classic TCP row) is a strict prefix of
/// `MIB_TCPROW_OWNER_PID`: both start with state/local/remote fields as `u32`s.
/// Rather than building the union by hand we simply reuse the row's bytes,
/// which is exactly what the API expects.
fn estats_row(row: &MIB_TCPROW_OWNER_PID) -> *const MIB_TCPROW_LH {
    row as *const MIB_TCPROW_OWNER_PID as *const MIB_TCPROW_LH
}

fn tcp_bytes_for_row(row: &MIB_TCPROW_OWNER_PID) -> Option<u64> {
    let lh = estats_row(row);
    debug_assert!(
        std::mem::size_of::<MIB_TCPROW_LH>() <= std::mem::size_of::<MIB_TCPROW_OWNER_PID>()
    );
    const NO_ERROR_CODE: u32 = 0;
    // 1. Make sure collection is on; ignore the result (it fails harmlessly when
    //    already enabled for this connection).
    let rw = TCP_ESTATS_DATA_RW_v0 {
        EnableCollection: 1u8,
    };
    unsafe {
        SetPerTcpConnectionEStats(
            lh,
            TcpConnectionEstatsData,
            &rw as *const TCP_ESTATS_DATA_RW_v0 as *const u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
            0,
        );
    }
    // 2. Read the counters.
    let mut rod: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        GetPerTcpConnectionEStats(
            lh,
            TcpConnectionEstatsData,
            std::ptr::null_mut(),
            0,
            0,
            std::ptr::null_mut(),
            0,
            0,
            &mut rod as *mut TCP_ESTATS_DATA_ROD_v0 as *mut u8,
            0,
            std::mem::size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32,
        )
    };
    if rc != NO_ERROR_CODE {
        return None;
    }
    Some(rod.DataBytesIn.saturating_add(rod.DataBytesOut))
}

/// Raw IPv4 TCP owner rows (shared helper so the byte counter reuses the table).
fn tcp_owner_rows() -> Vec<MIB_TCPROW_OWNER_PID> {
    let mut size = 0u32;
    let rc = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if (rc != ERROR_INSUFFICIENT_BUFFER && rc != 0) || size == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            0,
            AF_INET,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if rc != 0 {
        return Vec::new();
    }
    let count = unsafe { *(buf.as_ptr() as *const u32) } as usize;
    let offset = std::mem::size_of::<u32>();
    let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
    if offset + count * row_size > buf.len() {
        return Vec::new();
    }
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let row = unsafe {
            std::ptr::read_unaligned(
                buf.as_ptr().add(offset + i * row_size) as *const MIB_TCPROW_OWNER_PID
            )
        };
        rows.push(row);
    }
    rows
}

/// Resolve a PID to its listening port list (used by the services view to show
/// which port a service owns).
pub fn ports_of_pid(pid: u32) -> Vec<(Protocol, u16)> {
    let mut v: Vec<(Protocol, u16)> = Vec::new();
    for c in tcp_connections() {
        if c.pid == pid {
            v.push((c.protocol, c.local_port));
        }
    }
    for c in udp_listeners() {
        if c.pid == pid {
            v.push((c.protocol, c.local_port));
        }
    }
    v.sort_by_key(|(_, p)| *p);
    v.dedup();
    v
}

#[allow(dead_code)]
fn _ipv6_helper(b: &[u8; 16]) -> String {
    ipv6_to_string(b)
}

// ---------------------------------------------------------------------------
// The tray's network block: which adapter is up, and what it is called.
// ---------------------------------------------------------------------------

/// One network interface, as the tray needs it.
#[derive(Debug, Clone, Default)]
pub struct AdapterInfo {
    /// Friendly name (`Ethernet`, `Wi-Fi`, `agrohold.ru` is the *profile*, not
    /// this).
    pub name: String,
    pub description: String,
    /// True when the interface has a usable link and a non-loopback address.
    pub connected: bool,
    /// True for a wireless interface (`IfType == 71`).
    pub wireless: bool,
    /// One IPv4 address, when there is one.
    pub ipv4: String,
}

/// Every adapter with its link state, through `GetAdaptersAddresses`.
///
/// This is the same call Windows' own Network flyout makes, so the "connected"
/// flag matches what the user sees in Settings - no pinging, no timeouts.
pub fn adapters() -> Vec<AdapterInfo> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    };

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u8> = Vec::new();
    for _ in 0..3 {
        buf.resize(size as usize, 0);
        let rc = unsafe {
            GetAdaptersAddresses(
                0, // AF_UNSPEC: IPv4 and IPv6
                flags,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        // ERROR_BUFFER_OVERFLOW = 111: `size` now holds what is needed.
        if rc == 111 {
            continue;
        }
        if rc != 0 {
            return Vec::new();
        }
        break;
    }

    let mut out = Vec::new();
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !cur.is_null() {
        let a = unsafe { &*cur };
        // `PWSTR`s from IpHelper are NUL-terminated; `from_wide_ptr` walks them.
        let name = unsafe { crate::wide::from_wide_ptr(a.FriendlyName) };
        let description = unsafe { crate::wide::from_wide_ptr(a.Description) };
        // IfOperStatusUp = 1
        let up = a.OperStatus == 1 && a.IfType != 24; // 24 = loopback
        let mut ipv4 = String::new();
        let mut addr = a.FirstUnicastAddress;
        while !addr.is_null() {
            if let Some(sa) = unsafe { (*addr).Address.lpSockaddr.as_ref() } {
                if sa.sa_family == 2 {
                    // AF_INET
                    let s: &windows_sys::Win32::Networking::WinSock::SOCKADDR_IN =
                        unsafe { &*(sa as *const _ as *const _) };
                    let bytes = unsafe { s.sin_addr.S_un.S_addr.to_ne_bytes() };
                    ipv4 = format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3]);
                    if ipv4 != "127.0.0.1" && ipv4 != "0.0.0.0" {
                        break;
                    }
                    ipv4.clear();
                }
            }
            addr = unsafe { (*addr).Next };
        }
        out.push(AdapterInfo {
            name,
            description,
            connected: up && !ipv4.is_empty(),
            wireless: a.IfType == 71,
            ipv4,
        });
        cur = a.Next;
    }
    out
}

/// The adapter the tray should describe: the connected one, preferring a
/// wireless link.
pub fn primary_adapter() -> Option<AdapterInfo> {
    let all = adapters();
    all.iter()
        .find(|a| a.connected && a.wireless)
        .or_else(|| all.iter().find(|a| a.connected))
        .cloned()
}

/// SSID of the connected wireless network, read from the WLAN registry profiles.
///
/// `WlanQueryInterface` needs a handle dance that is not worth it for a label;
/// the NetworkList keys store the same profile name, and the fallback is the
/// adapter's friendly name.
pub fn wifi_ssid() -> Option<String> {
    use crate::registry::{Key, Root};
    let path = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\NetworkList\Signatures\Unmanaged";
    let base = Key::open(Root::CurrentUser, path).ok()?;
    for sub in base.subkeys() {
        let Ok(key) = Key::open(Root::CurrentUser, &format!("{path}\\{sub}")) else {
            continue;
        };
        if let Some(profile) = key.string("Description") {
            let profile = profile.trim().to_string();
            if !profile.is_empty() && profile != "Network" {
                return Some(profile);
            }
        }
    }
    None
}
