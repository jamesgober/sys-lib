//! Network on Linux: `/proc/net/dev` for the counters, `/proc/net/snmp` and
//! `/proc/net/sockstat(6)` for connection counts, and `/sys/class/net` for each
//! interface's details. Interface names are validated before they build a
//! path (no `/`, no `..`, at most 15 bytes).

use core::ops::ControlFlow;

use super::{Root, join_name};
use crate::{
    Bytes, Result,
    net::{Connections, Counters, InterfaceKind, MacKind, Totals},
    parse::{lossy, parse_u64, procfs, trim},
};

/// The longest interface name the kernel allows (`IFNAMSIZ` 16 with the NUL).
const IFNAMSIZ: usize = 15;

fn counters_of(l: &procfs::NetDevLine<'_>) -> Counters {
    Counters {
        interface: lossy(l.name, IFNAMSIZ),
        received: Bytes::new(l.rx_bytes),
        sent: Bytes::new(l.tx_bytes),
        packets_received: l.rx_packets,
        packets_sent: l.tx_packets,
        errors_in: l.rx_errors,
        errors_out: l.tx_errors,
        dropped_in: l.rx_dropped,
        dropped_out: l.tx_dropped,
    }
}

/// Every interface but loopback, summed, through the stack (no allocation).
pub(crate) fn totals(root: &Root) -> Result<Totals> {
    let mut t = Totals::default();
    root.for_each_line(
        "/proc/net/dev",
        super::FILE_LIMIT,
        "network counters",
        "/proc/net/dev",
        |line| {
            if let Some(l) = procfs::net_dev_line(line)
                && l.name != b"lo"
            {
                t.received = t.received.saturating_add(Bytes::new(l.rx_bytes));
                t.sent = t.sent.saturating_add(Bytes::new(l.tx_bytes));
                t.packets_received = t.packets_received.saturating_add(l.rx_packets);
                t.packets_sent = t.packets_sent.saturating_add(l.tx_packets);
                t.errors = t
                    .errors
                    .saturating_add(l.rx_errors)
                    .saturating_add(l.tx_errors);
                t.dropped = t
                    .dropped
                    .saturating_add(l.rx_dropped)
                    .saturating_add(l.tx_dropped);
            }
            ControlFlow::Continue(())
        },
    )?;
    Ok(t)
}

/// Each interface's counters, loopback included, at most `max`.
pub(crate) fn counters(root: &Root, max: usize) -> Result<Vec<Counters>> {
    let mut out = Vec::new();
    root.for_each_line(
        "/proc/net/dev",
        super::FILE_LIMIT,
        "network counters",
        "/proc/net/dev",
        |line| {
            if let Some(l) = procfs::net_dev_line(line) {
                if out.len() >= max {
                    return ControlFlow::Break(());
                }
                out.push(counters_of(&l));
            }
            ControlFlow::Continue(())
        },
    )?;
    Ok(out)
}

/// Connection counts from `/proc/net/snmp` and `/proc/net/sockstat(6)`. Each
/// value is `None` when its file or field is missing.
pub(crate) fn connections(root: &Root) -> Result<Connections> {
    let read = |path: &'static str, f: &dyn Fn(&[u8]) -> Option<u64>| {
        root.with(path, "connection counts", path, |d| f(d))
            .ok()
            .flatten()
    };
    let established = read("/proc/net/snmp", &|d| procfs::snmp(d, b"Tcp", b"CurrEstab"));
    let tcp4 = read("/proc/net/sockstat", &|d| {
        procfs::sockstat(d, b"TCP", b"inuse")
    });
    let time_wait = read("/proc/net/sockstat", &|d| {
        procfs::sockstat(d, b"TCP", b"tw")
    });
    let udp4 = read("/proc/net/sockstat", &|d| {
        procfs::sockstat(d, b"UDP", b"inuse")
    });
    let tcp6 = read("/proc/net/sockstat6", &|d| {
        procfs::sockstat(d, b"TCP6", b"inuse")
    });
    let udp6 = read("/proc/net/sockstat6", &|d| {
        procfs::sockstat(d, b"UDP6", b"inuse")
    });
    let sum = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    };
    Ok(Connections {
        tcp_established: established,
        tcp_time_wait: time_wait,
        tcp_total: sum(tcp4, tcp6),
        udp: sum(udp4, udp6),
    })
}

/// What `/sys/class/net/<name>` says about one interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Details {
    pub(crate) mac_kind: Option<MacKind>,
    pub(crate) kind: Option<InterfaceKind>,
    pub(crate) running: Option<bool>,
    pub(crate) mtu: Option<u32>,
    pub(crate) speed_mbps: Option<u64>,
    pub(crate) is_physical: bool,
}

/// Whether `name` may build a `/sys` path (E-55: names come from the OS, but
/// are still validated).
pub(crate) fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= IFNAMSIZ
        && name != b"."
        && name != b".."
        && !name.iter().any(|&b| b == b'/' || b == 0)
}

/// The interface kind from its ARPHRD type and the directories the kernel
/// creates for Wi-Fi and bridges.
pub(crate) fn kind_from(
    arphrd: u16,
    wireless: bool,
    bridge: bool,
    physical: bool,
) -> InterfaceKind {
    match arphrd {
        _ if wireless => InterfaceKind::Wifi,
        _ if bridge => InterfaceKind::Bridge,
        // ARPHRD_ETHER.
        1 if physical => InterfaceKind::Ethernet,
        1 => InterfaceKind::Virtual,
        // ARPHRD_LOOPBACK.
        772 => InterfaceKind::Loopback,
        // ARPHRD_NONE (tun, WireGuard), TUNNEL, TUNNEL6, IPGRE, SIT, IP6GRE.
        65534 | 768 | 769 | 776 | 778 | 823 => InterfaceKind::Tunnel,
        // ARPHRD_RAWIP (wwan modems).
        519 => InterfaceKind::Cellular,
        _ => InterfaceKind::Other,
    }
}

/// One interface's details from `/sys/class/net/<name>/`: MAC assignment type
/// (E-54), Wi-Fi and bridge markers, whether a device backs it, the
/// operational state, MTU, and speed (E-58: `-1` or an error is unknown).
pub(crate) fn details(root: &Root, name: &[u8], arphrd: Option<u16>) -> Details {
    if !valid_name(name) {
        return Details::default();
    }
    let mut buf = [0u8; 96];
    let mut text = |file: &str| -> Option<String> {
        let p = join_name(&mut buf, "/sys/class/net/", name, file)?;
        root.text(p)
    };
    let mac_kind = text("/addr_assign_type").and_then(|t| match t.as_str() {
        "0" => Some(MacKind::Permanent),
        "1" => Some(MacKind::Random),
        "2" | "3" => Some(MacKind::Unknown),
        _ => None,
    });
    let running = text("/operstate").map(|s| s == "up" || s == "unknown");
    let mtu = text("/mtu").and_then(|t| u32::try_from(parse_u64(t.as_bytes())?).ok());
    let speed_mbps = text("/speed")
        .and_then(|t| parse_u64(trim(t.as_bytes())))
        .filter(|&s| s > 0 && s < 4_000_000);
    let arphrd =
        arphrd.or_else(|| text("/type").and_then(|t| u16::try_from(parse_u64(t.as_bytes())?).ok()));
    let mut buf2 = [0u8; 96];
    let mut exists = |file: &str| {
        join_name(&mut buf2, "/sys/class/net/", name, file).is_some_and(|p| root.exists(p))
    };
    let wireless = exists("/wireless") || exists("/phy80211");
    let bridge = exists("/bridge");
    let is_physical = exists("/device");
    Details {
        mac_kind,
        kind: arphrd.map(|a| kind_from(a, wireless, bridge, is_physical)),
        running,
        mtu,
        speed_mbps,
        is_physical,
    }
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_net_totals_leave_out_loopback() {
        let root = fixtures::root("wsl2");
        let per = counters(&root, 4096).unwrap();
        assert!(
            per.iter().any(|c| c.interface == "lo") && per.iter().any(|c| c.interface == "eth0")
        );
        let t = totals(&root).unwrap();
        let eth0 = per.iter().find(|c| c.interface == "eth0").unwrap();
        let others: u64 = per
            .iter()
            .filter(|c| c.interface != "lo")
            .map(|c| c.received.raw())
            .sum();
        assert_eq!(t.received.raw(), others);
        assert!(t.received >= eth0.received);
        assert_eq!(counters(&root, 1).unwrap().len(), 1);
    }

    #[test]
    fn test_linux_net_connections_from_fixtures() {
        let c = connections(&fixtures::root("wsl2")).unwrap();
        assert!(c.tcp_established.is_some() && c.tcp_total.is_some() && c.udp.is_some());
        assert!(c.tcp_time_wait.is_some());
        let none = connections(&fixtures::root("no-such-fixture")).unwrap();
        assert_eq!(none, Connections::default());
    }

    #[test]
    fn test_linux_net_details_from_a_fixture() {
        let root = fixtures::root("wsl2");
        let d = details(&root, b"eth0", None);
        assert_eq!(d.kind, Some(InterfaceKind::Ethernet));
        assert!(d.is_physical && d.mtu.is_some());
        let lo = details(&root, b"lo", None);
        assert_eq!(lo.kind, Some(InterfaceKind::Loopback));
        assert_eq!(details(&root, b"../../etc", None), Details::default());
    }

    #[test]
    fn test_linux_net_names_and_kinds() {
        assert!(valid_name(b"eth0") && valid_name(b"wg-office"));
        assert!(!valid_name(b"a/b") && !valid_name(b"..") && !valid_name(b"0123456789abcdef"));
        assert_eq!(kind_from(1, true, false, true), InterfaceKind::Wifi);
        assert_eq!(kind_from(1, false, false, false), InterfaceKind::Virtual);
        assert_eq!(kind_from(65534, false, false, false), InterfaceKind::Tunnel);
    }
}
