//! Network: interfaces and their addresses, traffic counters, connection
//! counts, the default route, and the DNS configuration.

use std::{
    mem::{offset_of, size_of},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use super::{ffi, from_wide};
use crate::{
    Bytes, Error, Result,
    net::{
        Connections, Counters, Dns, Interface, InterfaceKind, InterfaceOptions, IpNet, Mac, Totals,
    },
};

/// The first `GetAdaptersAddresses` buffer: 15 KB is Microsoft's advice.
const ADAPTERS_START: usize = 16 * 1024;

/// The largest `GetAdaptersAddresses` buffer accepted.
const ADAPTERS_MAX: usize = 64 << 20;

/// The most addresses read from one interface.
const MAX_ADDRESSES: usize = 4_096;

/// The most UTF-16 units read from one adapter string.
const MAX_NAME: usize = 1_024;

/// The most rows read from `GetIfTable2`.
const MAX_ROWS: usize = 1 << 16;

/// The most adapters read for the DNS configuration.
const MAX_ADAPTERS: usize = 4_096;

/// The most DNS servers, and the most search domains.
const MAX_DNS: usize = 64;

/// `GetAdaptersAddresses` into an 8-byte-aligned buffer, asking for the size
/// and asking again at most 3 times. Empty when the machine has no adapters.
fn adapters(flags: u32, what: &'static str) -> Result<Vec<u64>> {
    let mut size = ADAPTERS_START;
    let mut code = 0;
    for _ in 0..3 {
        let mut buf = vec![0u64; size.div_ceil(8)];
        let mut len = u32::try_from(buf.len() * 8).unwrap_or(u32::MAX);
        // SAFETY: `buf` is writable for `len` bytes and 8-byte aligned, as the
        // adapter structs require; `len` is a writable local.
        code = unsafe {
            ffi::GetAdaptersAddresses(
                ffi::AF_UNSPEC,
                flags,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast::<ffi::IpAdapterAddresses>(),
                &raw mut len,
            )
        };
        match code {
            ffi::ERROR_SUCCESS => return Ok(buf),
            ffi::ERROR_NO_DATA => return Ok(Vec::new()),
            ffi::ERROR_BUFFER_OVERFLOW if (len as usize) <= ADAPTERS_MAX => {
                size = (len as usize).max(size);
            }
            _ => break,
        }
    }
    Err(Error::Os {
        what,
        call: "GetAdaptersAddresses",
        code: code as i32,
    })
}

/// A NUL-terminated UTF-16 string the OS returned, at most `MAX_NAME` units,
/// converted lossily (E-55).
///
/// # Safety
///
/// `p` is null or points to a NUL-terminated UTF-16 string that stays valid
/// for the call.
pub(super) unsafe fn string_at(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while len < MAX_NAME {
        let unit = p.wrapping_add(len);
        // SAFETY: the units up to the NUL are readable (the caller's
        // guarantee), and reading stops at the NUL.
        if unsafe { unit.read() } == 0 {
            break;
        }
        len += 1;
    }
    // SAFETY: the `len` units just read are initialised and in bounds.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// The address in a `SOCKADDR_IN` or `SOCKADDR_IN6`, from its bytes.
fn ip_from_sockaddr(bytes: &[u8]) -> Option<IpAddr> {
    let family = u16::from_ne_bytes([*bytes.first()?, *bytes.get(1)?]);
    match family {
        ffi::AF_INET => {
            let at = offset_of!(ffi::SockaddrIn, addr);
            let a: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(a)))
        }
        ffi::AF_INET6 => {
            let at = offset_of!(ffi::SockaddrIn6, addr);
            let a: [u8; 16] = bytes.get(at..at + 16)?.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(a)))
        }
        _ => None,
    }
}

/// The bytes a `SOCKET_ADDRESS` points at, at most a `SOCKADDR_IN6`.
///
/// # Safety
///
/// `a` comes from a live `GetAdaptersAddresses` buffer: `sockaddr` is null or
/// points at `sockaddr_length` readable bytes.
unsafe fn sockaddr_bytes(a: &ffi::SocketAddress) -> &[u8] {
    let len = usize::try_from(a.sockaddr_length)
        .unwrap_or(0)
        .min(size_of::<ffi::SockaddrIn6>());
    if a.sockaddr.is_null() || len == 0 {
        return &[];
    }
    // SAFETY: the caller's guarantee covers `len` bytes from `sockaddr`.
    unsafe { std::slice::from_raw_parts(a.sockaddr.cast::<u8>(), len) }
}

/// The interface kind of an IANA `ifType`.
fn kind_of(if_type: u32) -> InterfaceKind {
    match if_type {
        ffi::IF_TYPE_ETHERNET_CSMACD => InterfaceKind::Ethernet,
        ffi::IF_TYPE_IEEE80211 => InterfaceKind::Wifi,
        ffi::IF_TYPE_SOFTWARE_LOOPBACK => InterfaceKind::Loopback,
        ffi::IF_TYPE_TUNNEL => InterfaceKind::Tunnel,
        ffi::IF_TYPE_WWANPP | ffi::IF_TYPE_WWANPP2 => InterfaceKind::Cellular,
        _ => InterfaceKind::Other,
    }
}

/// A link speed in bits per second as megabits; the maximum values and zero
/// mean unknown (E-58).
fn speed_mbps(bits: u64) -> Option<u64> {
    (bits != 0 && bits != u64::MAX && bits != u64::from(u32::MAX)).then_some(bits / 1_000_000)
}

/// A 6-byte hardware address; none for other lengths or all zeros (E-53).
fn mac_of(bytes: &[u8; 8], len: u32) -> Option<Mac> {
    let six: [u8; 6] = bytes[..6].try_into().ok()?;
    (len == 6 && six != [0; 6]).then(|| Mac::from_bytes(six))
}

/// Every adapter `GetAdaptersAddresses` reports. With `details`, the MTU and
/// speed are filled in, and the interface table (`GetIfTable2`) gives the
/// administrative state and whether the adapter is hardware: `is_physical`
/// is an Ethernet or Wi-Fi adapter the OS marks as hardware, which leaves out
/// Hyper-V and VPN adapters. Without `details`, `up` follows the operational
/// state and `is_physical` is false.
pub(crate) fn interfaces(options: &InterfaceOptions) -> Result<Vec<Interface>> {
    let flags = ffi::GAA_FLAG_INCLUDE_PREFIX
        | ffi::GAA_FLAG_SKIP_ANYCAST
        | ffi::GAA_FLAG_SKIP_MULTICAST
        | ffi::GAA_FLAG_SKIP_DNS_SERVER;
    let buf = adapters(flags, "network interfaces")?;
    let table = if options.details {
        IfTable::read().ok()
    } else {
        None
    };
    let rows = table.as_ref().map_or(&[][..], IfTable::rows);
    let mut out = Vec::new();
    let mut p = if buf.is_empty() {
        std::ptr::null()
    } else {
        buf.as_ptr().cast::<ffi::IpAdapterAddresses>()
    };
    while !p.is_null() {
        if out.len() >= options.max_interfaces {
            return Err(Error::TooLarge {
                what: "network interfaces",
                limit: options.max_interfaces as u64,
            });
        }
        // SAFETY: `p` is the head of the list GetAdaptersAddresses wrote into
        // `buf`, or a `next` link of it: an aligned, initialised entry that
        // lives as long as `buf`.
        let a = unsafe { &*p };
        out.push(interface(a, rows, options.details));
        p = a.next;
    }
    Ok(out)
}

/// The interface the route to anywhere leaves by: the default route, IPv4.
/// No route is `None`; a machine with an IPv6 default route alone has none here.
pub(crate) fn primary_interface() -> Result<Option<u32>> {
    let anywhere = ffi::SockaddrIn {
        family: ffi::AF_INET,
        port: 0,
        addr: 0,
        zero: [0; 8],
    };
    let mut index = 0u32;
    // SAFETY: `anywhere` is a whole SOCKADDR_IN, read by its family; `index` is
    // a writable local.
    let code = unsafe {
        ffi::GetBestInterfaceEx(
            (&raw const anywhere).cast::<ffi::Sockaddr>(),
            &raw mut index,
        )
    };
    match code {
        ffi::ERROR_SUCCESS => Ok(Some(index)),
        ffi::ERROR_NOT_FOUND
        | ffi::ERROR_NETWORK_UNREACHABLE
        | ffi::ERROR_HOST_UNREACHABLE
        | ffi::ERROR_NO_DATA => Ok(None),
        _ => Err(Error::Os {
            what: "default route",
            call: "GetBestInterfaceEx",
            code: code as i32,
        }),
    }
}

/// An adapter's rank among the DNS sources: its lower interface metric. A
/// family the adapter does not run has metric 0, which says nothing.
fn metric_rank(ipv4: u32, ipv6: u32) -> u32 {
    [ipv4, ipv6]
        .into_iter()
        .filter(|&m| m != 0)
        .min()
        .unwrap_or(u32::MAX)
}

/// The deprecated site-local addresses Windows lists on an adapter with no
/// IPv6 DNS server configured, `fec0:0:0:ffff::1` to `::3`. Nothing answers on
/// them.
fn is_placeholder_server(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[..4] == [0xfec0, 0, 0, 0xffff] && s[4..7] == [0, 0, 0] && matches!(s[7], 1..=3)
        }
        IpAddr::V4(_) => false,
    }
}

/// Adds `item` once; `TooLarge` past [`MAX_DNS`].
fn add_once<T: PartialEq>(list: &mut Vec<T>, item: T) -> Result<()> {
    if list.contains(&item) {
        return Ok(());
    }
    if list.len() == MAX_DNS {
        return Err(Error::TooLarge {
            what: "DNS configuration",
            limit: MAX_DNS as u64,
        });
    }
    list.push(item);
    Ok(())
}

/// Adds one adapter's servers and search domains.
fn add_dns(a: &ffi::IpAdapterAddresses, dns: &mut Dns) -> Result<()> {
    let mut server = a.first_dns_server_address.cast_const();
    for _ in 0..MAX_ADDRESSES {
        if server.is_null() {
            break;
        }
        // SAFETY: an entry of the server list inside the same buffer as `a`.
        let entry = unsafe { &*server };
        // SAFETY: the address belongs to that live buffer.
        let bytes = unsafe { sockaddr_bytes(&entry.address) };
        if let Some(ip) = ip_from_sockaddr(bytes).filter(|&ip| !is_placeholder_server(ip)) {
            add_once(&mut dns.servers, ip)?;
        }
        server = entry.next.cast_const();
    }
    // SAFETY: the adapter's strings live in the same buffer as `a`.
    let suffix = unsafe { string_at(a.dns_suffix) };
    if !suffix.is_empty() {
        add_once(&mut dns.search, suffix)?;
    }
    if (a.length as usize) < size_of::<ffi::IpAdapterAddresses>() {
        return Ok(());
    }
    let mut suffix = a.first_dns_suffix.cast_const();
    for _ in 0..MAX_ADDRESSES {
        if suffix.is_null() {
            break;
        }
        // SAFETY: an entry of the suffix list inside the same buffer as `a`.
        let entry = unsafe { &*suffix };
        let text = from_wide(&entry.string);
        if !text.is_empty() {
            add_once(&mut dns.search, text)?;
        }
        suffix = entry.next.cast_const();
    }
    Ok(())
}

/// The DNS servers and suffixes of the adapters that are up, lowest metric
/// first, each once. Loopback has none to give.
pub(crate) fn dns() -> Result<Dns> {
    let flags =
        ffi::GAA_FLAG_SKIP_UNICAST | ffi::GAA_FLAG_SKIP_ANYCAST | ffi::GAA_FLAG_SKIP_MULTICAST;
    let buf = adapters(flags, "DNS configuration")?;
    let mut up = Vec::new();
    let mut p = if buf.is_empty() {
        std::ptr::null()
    } else {
        buf.as_ptr().cast::<ffi::IpAdapterAddresses>()
    };
    for _ in 0..MAX_ADAPTERS {
        if p.is_null() {
            break;
        }
        // SAFETY: as in `interfaces`: the head of the list in `buf`, or a `next`
        // link of it, aligned, initialised, and as long-lived as `buf`.
        let a = unsafe { &*p };
        if a.oper_status == ffi::IF_OPER_STATUS_UP && a.if_type != ffi::IF_TYPE_SOFTWARE_LOOPBACK {
            up.push(a);
        }
        p = a.next;
    }
    up.sort_by_key(|a| metric_rank(a.ipv4_metric, a.ipv6_metric));
    let mut dns = Dns::default();
    for a in up {
        add_dns(a, &mut dns)?;
    }
    Ok(dns)
}

fn interface(a: &ffi::IpAdapterAddresses, rows: &[ffi::MibIfRow2], details: bool) -> Interface {
    let row = rows.iter().find(|r| r.interface_luid == a.luid);
    let kind = kind_of(a.if_type);
    let running = a.oper_status == ffi::IF_OPER_STATUS_UP;
    let mut addresses = Vec::new();
    let mut u = a.first_unicast_address.cast_const();
    while !u.is_null() && addresses.len() < MAX_ADDRESSES {
        // SAFETY: an entry of the unicast list inside the same buffer as `a`.
        let entry = unsafe { &*u };
        // SAFETY: the address belongs to that live buffer.
        let bytes = unsafe { sockaddr_bytes(&entry.address) };
        if let Some(addr) = ip_from_sockaddr(bytes) {
            addresses.push(IpNet {
                addr,
                prefix: entry.on_link_prefix_length,
            });
        }
        u = entry.next.cast_const();
    }
    // SAFETY: the adapter's strings live in the same buffer as `a`.
    let name = unsafe { string_at(a.friendly_name) };
    // SAFETY: as above.
    let description = unsafe { string_at(a.description) };
    Interface {
        name,
        index: if a.if_index != 0 {
            a.if_index
        } else {
            a.ipv6_if_index
        },
        description: Some(description).filter(|d| !d.is_empty()),
        kind,
        mac: mac_of(&a.physical_address, a.physical_address_length),
        addresses,
        up: row.map_or(running, |r| r.admin_status == ffi::NET_IF_ADMIN_STATUS_UP),
        running,
        mtu: Some(a.mtu).filter(|&m| details && m != 0 && m != u32::MAX),
        speed_mbps: speed_mbps(a.transmit_link_speed).filter(|_| details),
        is_physical: matches!(kind, InterfaceKind::Ethernet | InterfaceKind::Wifi)
            && a.tunnel_type == ffi::TUNNEL_TYPE_NONE
            && row.is_some_and(|r| {
                r.interface_and_oper_status_flags & ffi::IF_FLAG_HARDWARE_INTERFACE != 0
            }),
    }
}

/// The interface table from `GetIfTable2`, freed on drop.
struct IfTable(*mut ffi::MibIfTable2);

impl IfTable {
    fn read() -> Result<IfTable> {
        let mut table = std::ptr::null_mut();
        // SAFETY: `table` is a writable pointer slot the call fills.
        let code = unsafe { ffi::GetIfTable2(&raw mut table) };
        if code != ffi::ERROR_SUCCESS || table.is_null() {
            return Err(Error::Os {
                what: "network counters",
                call: "GetIfTable2",
                code: code as i32,
            });
        }
        Ok(IfTable(table))
    }

    fn rows(&self) -> &[ffi::MibIfRow2] {
        // SAFETY: the table is live until drop; its header is initialised.
        let n = unsafe { (*self.0).num_entries } as usize;
        // SAFETY: as above; the rows start at `table`, taken as a raw place so
        // the pointer keeps the whole allocation's provenance.
        let first = unsafe { &raw const (*self.0).table }.cast::<ffi::MibIfRow2>();
        // SAFETY: `num_entries` initialised rows follow the header.
        unsafe { std::slice::from_raw_parts(first, n.min(MAX_ROWS)) }
    }
}

impl Drop for IfTable {
    fn drop(&mut self) {
        // SAFETY: the table came from GetIfTable2 and is freed once.
        unsafe { ffi::FreeMibTable(self.0.cast_const().cast()) };
    }
}

/// The rows that are interfaces of their own: not the filter and sub-layer
/// rows (`... -WFP Native MAC Layer LightWeight Filter-0000`, `... -QoS Packet
/// Scheduler-0000`) that repeat an adapter's traffic. Software loopback stays.
fn is_interface_row(r: &ffi::MibIfRow2) -> bool {
    r.interface_and_oper_status_flags & ffi::IF_FLAG_FILTER_INTERFACE == 0
}

/// The rows counted in the totals: hardware interfaces, or ones with a
/// connector present, never software loopback.
fn is_counted_row(r: &ffi::MibIfRow2) -> bool {
    let flags = r.interface_and_oper_status_flags;
    is_interface_row(r)
        && r.if_type != ffi::IF_TYPE_SOFTWARE_LOOPBACK
        && flags & (ffi::IF_FLAG_HARDWARE_INTERFACE | ffi::IF_FLAG_CONNECTOR_PRESENT) != 0
}

fn counters_of(r: &ffi::MibIfRow2, interface: String) -> Counters {
    Counters {
        interface,
        received: Bytes::new(r.in_octets),
        sent: Bytes::new(r.out_octets),
        packets_received: r.in_ucast_pkts.saturating_add(r.in_n_ucast_pkts),
        packets_sent: r.out_ucast_pkts.saturating_add(r.out_n_ucast_pkts),
        errors_in: r.in_errors,
        errors_out: r.out_errors,
        dropped_in: r.in_discards,
        dropped_out: r.out_discards,
    }
}

/// Hardware interfaces and those with a connector present, added up; nothing
/// allocated on this side (the OS allocates the table).
pub(crate) fn net_totals() -> Result<Totals> {
    let table = IfTable::read()?;
    let mut totals = Totals::default();
    for row in table.rows().iter().filter(|r| is_counted_row(r)) {
        totals.add(&counters_of(row, String::new()));
    }
    Ok(totals)
}

/// Each interface's counters, named by its alias (the friendly name).
pub(crate) fn net_counters() -> Result<Vec<Counters>> {
    let table = IfTable::read()?;
    Ok(table
        .rows()
        .iter()
        .filter(|r| is_interface_row(r))
        .map(|r| counters_of(r, super::from_wide(&r.alias)))
        .collect())
}

/// TCP and UDP counts over IPv4 and IPv6. A family the machine lacks is left
/// out; time-wait is not reported.
pub(crate) fn connections() -> Result<Connections> {
    let mut c = Connections::default();
    let mut failed = None;
    for family in [ffi::AF_INET, ffi::AF_INET6] {
        let mut tcp = ffi::MibTcpStats::default();
        // SAFETY: `tcp` is a writable MIB_TCPSTATS.
        let code = unsafe { ffi::GetTcpStatisticsEx(&raw mut tcp, u32::from(family)) };
        if code == ffi::ERROR_SUCCESS {
            add(&mut c.tcp_established, tcp.curr_estab);
            add(&mut c.tcp_total, tcp.num_conns);
        } else {
            failed = Some(("GetTcpStatisticsEx", code));
        }
        let mut udp = ffi::MibUdpStats::default();
        // SAFETY: `udp` is a writable MIB_UDPSTATS.
        let code = unsafe { ffi::GetUdpStatisticsEx(&raw mut udp, u32::from(family)) };
        if code == ffi::ERROR_SUCCESS {
            add(&mut c.udp, udp.num_addrs);
        } else {
            failed = Some(("GetUdpStatisticsEx", code));
        }
    }
    match failed {
        Some((call, code)) if c.tcp_total.is_none() && c.udp.is_none() => Err(Error::Os {
            what: "connection counts",
            call,
            code: code as i32,
        }),
        _ => Ok(c),
    }
}

fn add(total: &mut Option<u64>, n: u32) {
    *total = Some(total.unwrap_or(0).saturating_add(u64::from(n)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_net_windows_sockaddr_v4_v6_and_junk() {
        let mut v4 = [0u8; 16];
        v4[..2].copy_from_slice(&ffi::AF_INET.to_ne_bytes());
        v4[4..8].copy_from_slice(&[192, 168, 1, 20]);
        assert_eq!(
            ip_from_sockaddr(&v4),
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)))
        );
        let mut v6 = [0u8; 28];
        v6[..2].copy_from_slice(&ffi::AF_INET6.to_ne_bytes());
        v6[8] = 0xfe;
        v6[9] = 0x80;
        v6[23] = 1;
        assert_eq!(
            ip_from_sockaddr(&v6),
            Some(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)))
        );
        // Truncated, empty, or another family.
        assert_eq!(ip_from_sockaddr(&v6[..20]), None);
        assert_eq!(ip_from_sockaddr(&v4[..6]), None);
        assert_eq!(ip_from_sockaddr(&[]), None);
        assert_eq!(ip_from_sockaddr(&[1, 0, 0, 0]), None);
    }

    #[test]
    fn test_net_windows_kind_speed_and_mac() {
        assert_eq!(kind_of(6), InterfaceKind::Ethernet);
        assert_eq!(kind_of(71), InterfaceKind::Wifi);
        assert_eq!(kind_of(24), InterfaceKind::Loopback);
        assert_eq!(kind_of(131), InterfaceKind::Tunnel);
        assert_eq!(kind_of(243), InterfaceKind::Cellular);
        assert_eq!(kind_of(244), InterfaceKind::Cellular);
        assert_eq!(kind_of(23), InterfaceKind::Other);
        assert_eq!(speed_mbps(1_000_000_000), Some(1_000));
        assert_eq!(speed_mbps(u64::MAX), None);
        assert_eq!(speed_mbps(u64::from(u32::MAX)), None);
        assert_eq!(speed_mbps(0), None);
        let bytes = [0x00, 0x1b, 0x21, 0xaa, 0xbb, 0x0c, 0, 0];
        assert_eq!(mac_of(&bytes, 6).unwrap().to_string(), "00:1b:21:aa:bb:0c");
        assert_eq!(mac_of(&bytes, 8), None);
        assert_eq!(mac_of(&[0; 8], 6), None);
    }

    #[test]
    fn test_net_windows_dns_rules() {
        assert_eq!(metric_rank(25, 35), 25);
        assert_eq!(metric_rank(0, 35), 35);
        assert_eq!(metric_rank(25, 0), 25);
        assert_eq!(metric_rank(0, 0), u32::MAX);
        let site = |last| IpAddr::V6(Ipv6Addr::new(0xfec0, 0, 0, 0xffff, 0, 0, 0, last));
        assert!(is_placeholder_server(site(1)) && is_placeholder_server(site(3)));
        assert!(!is_placeholder_server(site(4)));
        assert!(!is_placeholder_server(IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
        assert!(!is_placeholder_server(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0xffff, 0, 0, 0, 1
        ))));
        let mut list = Vec::new();
        for i in 0..MAX_DNS {
            add_once(&mut list, i).unwrap();
            add_once(&mut list, i).unwrap();
        }
        assert_eq!(list.len(), MAX_DNS);
        add_once(&mut list, 0).unwrap();
        assert!(matches!(
            add_once(&mut list, MAX_DNS),
            Err(Error::TooLarge { limit: 64, .. })
        ));
    }

    #[test]
    fn test_net_windows_live_default_route_and_dns() {
        if let Some(index) = primary_interface().unwrap() {
            let list = interfaces(&InterfaceOptions::default()).unwrap();
            assert!(list.iter().any(|i| i.index == index), "{index}");
        }
        let d = dns().unwrap();
        assert!(d.servers.len() <= MAX_DNS && d.search.len() <= MAX_DNS);
        assert!(d.upstream.is_empty());
        assert!(d.servers.iter().all(|&ip| !is_placeholder_server(ip)));
    }

    #[test]
    fn test_net_windows_live_interfaces_counters_connections() {
        let list = interfaces(&InterfaceOptions::default()).unwrap();
        assert!(!list.is_empty());
        assert!(list.iter().any(|i| i.kind == InterfaceKind::Loopback));
        for i in &list {
            assert!(!i.name.is_empty(), "{i:?}");
            assert!(
                !i.is_physical || matches!(i.kind, InterfaceKind::Ethernet | InterfaceKind::Wifi)
            );
            assert!(i.addresses.iter().all(|a| a.prefix <= 128), "{i:?}");
        }
        let quick = interfaces(&InterfaceOptions {
            details: false,
            ..InterfaceOptions::default()
        })
        .unwrap();
        assert_eq!(quick.len(), list.len());
        assert!(quick.iter().all(|i| i.mtu.is_none() && !i.is_physical));
        let none = interfaces(&InterfaceOptions {
            max_interfaces: 0,
            ..InterfaceOptions::default()
        });
        assert!(matches!(none, Err(Error::TooLarge { .. })));

        let counters = net_counters().unwrap();
        assert!(!counters.is_empty());
        assert!(
            counters.iter().all(|c| !c.interface.contains("-0000")),
            "{counters:?}"
        );
        let t = net_totals().unwrap();
        let sum: u64 = counters.iter().map(|c| c.received.raw()).sum();
        assert!(t.received.raw() <= sum + (1 << 30));
        let c = connections().unwrap();
        assert!(c.tcp_total.is_some() && c.udp.is_some() && c.tcp_time_wait.is_none());
        assert!(c.tcp_established <= c.tcp_total);
    }
}
