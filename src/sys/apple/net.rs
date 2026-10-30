//! Network: `getifaddrs` for interfaces and addresses, the `NET_RT_IFLIST2`
//! routing sysctl for 64-bit counters and link details, and the PCB counts.
//!
//! Every byte the OS hands back is copied out and parsed by safe code over
//! slices: the routing-message walker and the `sockaddr` readers below.

use core::{
    ffi::{CStr, c_void},
    mem::{offset_of, size_of},
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::{bytes_at, ffi, i32_at, last_errno, os_error, u16_at, u32_at, u64_at};
use crate::{
    Bytes, Error, Result,
    net::{Connections, Counters, Interface, InterfaceKind, InterfaceOptions, IpNet, Mac, Totals},
};

// ---------------------------------------------------------------------------
// The routing sysctl
// ---------------------------------------------------------------------------

/// Routing messages fit this stack buffer on ordinary machines (about 30
/// interfaces), so the counters allocate nothing.
const STACK_LIST: usize = 32 << 10;
/// The most routing-message bytes read; more is `TooLarge`.
const MAX_LIST: usize = 16 << 20;
const IFLIST2: &str = "sysctl NET_RT_IFLIST2";

/// `sysctl(CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0)` into `buf`, or the size
/// it needs with no buffer: the bytes, or the `errno`.
fn iflist2(buf: Option<&mut [u8]>) -> core::result::Result<usize, i32> {
    let mut mib = [ffi::CTL_NET, ffi::PF_ROUTE, 0, 0, ffi::NET_RT_IFLIST2, 0];
    let (ptr, mut len) = match buf {
        Some(b) => (b.as_mut_ptr().cast::<c_void>(), b.len()),
        None => (core::ptr::null_mut(), 0),
    };
    // SAFETY: `mib` is six writable integers; `ptr` is null (a size query) or
    // `len` writable bytes, of which the kernel writes at most `len`, updating
    // it.
    let rc = unsafe {
        ffi::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            ptr,
            &raw mut len,
            core::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(last_errno());
    }
    Ok(len)
}

/// Calls `f` with the `NET_RT_IFLIST2` messages: in a stack buffer when they fit,
/// else in a heap buffer bounded at 16 MiB. Interfaces appearing between the
/// size query and the read (`ENOMEM`) mean asking again, at most three times.
fn with_iflist2<R>(what: &'static str, f: impl FnOnce(&[u8]) -> Result<R>) -> Result<R> {
    let mut stack = [0u8; STACK_LIST];
    let mut heap: Vec<u8> = Vec::new();
    for _ in 0..3 {
        let need = iflist2(None).map_err(|code| os_error(what, IFLIST2, code))?;
        let buf: &mut [u8] = if need <= STACK_LIST {
            &mut stack
        } else {
            let size = need.saturating_add(need / 8).saturating_add(4096);
            if size > MAX_LIST {
                return Err(Error::TooLarge {
                    what,
                    limit: MAX_LIST as u64,
                });
            }
            heap.resize(size, 0);
            &mut heap
        };
        match iflist2(Some(buf)) {
            Ok(len) => return f(&buf[..len.min(buf.len())]),
            Err(ffi::ENOMEM) => {}
            Err(code) => return Err(os_error(what, IFLIST2, code)),
        }
    }
    Err(Error::Os {
        what,
        call: IFLIST2,
        code: ffi::ENOMEM,
    })
}

/// One interface's `RTM_IFINFO2` message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IfInfo<'a> {
    index: u16,
    flags: i32,
    /// From the `sockaddr_dl` after the header.
    name: Option<&'a [u8]>,
    ftype: u8,
    mtu: u32,
    baudrate: u64,
    ipackets: u64,
    ierrors: u64,
    opackets: u64,
    oerrors: u64,
    ibytes: u64,
    obytes: u64,
    iqdrops: u64,
    snd_drops: u64,
}

impl IfInfo<'_> {
    fn is_loopback(&self) -> bool {
        self.flags.cast_unsigned() & ffi::IFF_LOOPBACK != 0
    }

    fn counters(&self, name: &[u8]) -> Counters {
        Counters {
            interface: crate::parse::lossy(name, 64),
            received: Bytes::new(self.ibytes),
            sent: Bytes::new(self.obytes),
            packets_received: self.ipackets,
            packets_sent: self.opackets,
            errors_in: self.ierrors,
            errors_out: self.oerrors,
            dropped_in: self.iqdrops,
            dropped_out: self.snd_drops,
        }
    }
}

/// Walks the routing messages, calling `f` for each interface (`RTM_IFINFO2`
/// at `RTM_VERSION`); other messages (the addresses) are skipped. Each message
/// is at least 4 bytes, so the walk is bounded by the buffer.
fn walk_iflist2<'a>(
    what: &'static str,
    buf: &'a [u8],
    mut f: impl FnMut(IfInfo<'a>),
) -> Result<()> {
    let malformed = || Error::Malformed {
        what,
        source: IFLIST2,
        line: None,
    };
    let mut at = 0;
    while at < buf.len() {
        let rest = &buf[at..];
        let len =
            usize::from(u16_at(rest, offset_of!(ffi::IfMsghdr2, msglen)).ok_or_else(malformed)?);
        if len < 4 || len > rest.len() {
            return Err(malformed());
        }
        let msg = &rest[..len];
        if msg[offset_of!(ffi::IfMsghdr2, version)] == ffi::RTM_VERSION
            && msg[offset_of!(ffi::IfMsghdr2, mtype)] == ffi::RTM_IFINFO2
        {
            f(parse_ifinfo2(msg).ok_or_else(malformed)?);
        }
        at += len;
    }
    Ok(())
}

/// The fields of one `if_msghdr2` and the name in the `sockaddr_dl` after it.
fn parse_ifinfo2(msg: &[u8]) -> Option<IfInfo<'_>> {
    const HEADER: usize = size_of::<ffi::IfMsghdr2>();
    const DATA: usize = offset_of!(ffi::IfMsghdr2, data);
    if msg.len() < HEADER {
        return None;
    }
    let data = |field: usize| u64_at(msg, DATA + field);
    let addrs = i32_at(msg, offset_of!(ffi::IfMsghdr2, addrs))?;
    let name = if addrs & ffi::RTA_IFP != 0 {
        link_name(&msg[HEADER..])
    } else {
        None
    };
    Some(IfInfo {
        index: u16_at(msg, offset_of!(ffi::IfMsghdr2, index))?,
        flags: i32_at(msg, offset_of!(ffi::IfMsghdr2, flags))?,
        name,
        ftype: *msg.get(DATA + offset_of!(ffi::IfData64, ftype))?,
        mtu: u32_at(msg, DATA + offset_of!(ffi::IfData64, mtu))?,
        baudrate: data(offset_of!(ffi::IfData64, baudrate))?,
        ipackets: data(offset_of!(ffi::IfData64, ipackets))?,
        ierrors: data(offset_of!(ffi::IfData64, ierrors))?,
        opackets: data(offset_of!(ffi::IfData64, opackets))?,
        oerrors: data(offset_of!(ffi::IfData64, oerrors))?,
        ibytes: data(offset_of!(ffi::IfData64, ibytes))?,
        obytes: data(offset_of!(ffi::IfData64, obytes))?,
        iqdrops: data(offset_of!(ffi::IfData64, iqdrops))?,
        snd_drops: u64::try_from(i32_at(msg, offset_of!(ffi::IfMsghdr2, snd_drops))?).unwrap_or(0),
    })
}

/// The interface name in a `sockaddr_dl`: the first `sdl_nlen` bytes of
/// `sdl_data`, inside both `sdl_len` and the slice.
fn link_name(sdl: &[u8]) -> Option<&[u8]> {
    let len = usize::from(*sdl.first()?).min(sdl.len());
    let sdl = &sdl[..len];
    if *sdl.get(offset_of!(ffi::SockaddrDl, family))? != ffi::AF_LINK {
        return None;
    }
    let start = offset_of!(ffi::SockaddrDl, data);
    let nlen = usize::from(*sdl.get(offset_of!(ffi::SockaddrDl, nlen))?);
    let name = sdl.get(start..start.checked_add(nlen)?)?;
    (!name.is_empty()).then_some(name)
}

pub(crate) fn net_totals() -> Result<Totals> {
    const WHAT: &str = "network counters";
    with_iflist2(WHAT, |buf| {
        let mut t = Totals::default();
        walk_iflist2(WHAT, buf, |i| {
            // An empty name allocates nothing: the totals stay allocation-free.
            if !i.is_loopback() {
                t.add(&i.counters(b""));
            }
        })?;
        Ok(t)
    })
}

pub(crate) fn net_counters() -> Result<Vec<Counters>> {
    const WHAT: &str = "network counters";
    with_iflist2(WHAT, |buf| {
        let mut out = Vec::new();
        walk_iflist2(WHAT, buf, |i| {
            // Every interface message carries its name; one without is skipped
            // here (the totals still count it).
            if let Some(name) = i.name {
                out.push(i.counters(name));
            }
        })?;
        Ok(out)
    })
}

/// The PCB counts: every TCP socket (listening included) and every UDP socket,
/// IPv4 and IPv6 (one table each). Established and time-wait are not counted
/// separately by any public sysctl.
pub(crate) fn connections() -> Result<Connections> {
    Ok(Connections {
        tcp_established: None,
        tcp_time_wait: None,
        tcp_total: super::sysctl_u64(c"net.inet.tcp.pcbcount"),
        udp: super::sysctl_u64(c"net.inet.udp.pcbcount"),
    })
}

// ---------------------------------------------------------------------------
// Interfaces
// ---------------------------------------------------------------------------

/// The list `getifaddrs` returns, freed on drop.
struct IfAddrs(*mut ffi::Ifaddrs);

impl IfAddrs {
    fn new() -> Result<IfAddrs> {
        let mut list: *mut ffi::Ifaddrs = core::ptr::null_mut();
        // SAFETY: `list` is writable; on success the list is owned by the
        // returned value.
        let rc = unsafe { ffi::getifaddrs(&raw mut list) };
        if rc != 0 {
            return Err(os_error("network interfaces", "getifaddrs", last_errno()));
        }
        Ok(IfAddrs(list))
    }

    /// The entries, borrowed from the list.
    fn entries(&self) -> impl Iterator<Item = &ffi::Ifaddrs> {
        // SAFETY: the head is null or a live entry of the list `self` owns.
        let head = unsafe { self.0.as_ref() };
        core::iter::successors(head, |entry| {
            // SAFETY: `next` is null or a live entry of the same list.
            unsafe { entry.next.as_ref() }
        })
    }
}

impl Drop for IfAddrs {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the list came from `getifaddrs` and is freed once.
            unsafe { ffi::freeifaddrs(self.0) };
        }
    }
}

/// Copies a `sockaddr` the OS returned into `out`: its `sa_len` bytes (a
/// netmask may be trimmed shorter than its type). The length copied.
///
/// # Safety
///
/// `sa` is null or points to at least `sa_len` readable bytes.
unsafe fn copy_sockaddr(sa: *const ffi::Sockaddr, out: &mut [u8; 256]) -> usize {
    if sa.is_null() {
        return 0;
    }
    // SAFETY: the caller's contract: `sa` points to a `sockaddr`, whose first
    // byte is `sa_len`.
    let len = usize::from(unsafe { sa.cast::<u8>().read() });
    // SAFETY: the caller's contract: `len` bytes are readable; `out` holds 256,
    // more than any `u8` length.
    unsafe { core::ptr::copy_nonoverlapping(sa.cast::<u8>(), out.as_mut_ptr(), len) };
    len
}

/// What one `getifaddrs` address is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sock {
    Ip(IpAddr),
    Link {
        index: u16,
        ftype: u8,
        mac: Option<[u8; 6]>,
    },
}

/// A `sockaddr_in`, `sockaddr_in6`, or `sockaddr_dl` from its bytes.
fn parse_sockaddr(sa: &[u8]) -> Option<Sock> {
    match *sa.get(offset_of!(ffi::Sockaddr, family))? {
        ffi::AF_INET => {
            let a: [u8; 4] = bytes_at(sa, offset_of!(ffi::SockaddrIn, addr))?;
            Some(Sock::Ip(IpAddr::V4(Ipv4Addr::from(a))))
        }
        ffi::AF_INET6 => {
            let a: [u8; 16] = bytes_at(sa, offset_of!(ffi::SockaddrIn6, addr))?;
            Some(Sock::Ip(IpAddr::V6(Ipv6Addr::from(clear_embedded_scope(
                a,
            )))))
        }
        ffi::AF_LINK => {
            let nlen = usize::from(*sa.get(offset_of!(ffi::SockaddrDl, nlen))?);
            let alen = usize::from(*sa.get(offset_of!(ffi::SockaddrDl, alen))?);
            let mac = if alen == 6 {
                bytes_at::<6>(sa, offset_of!(ffi::SockaddrDl, data) + nlen)
                    .filter(|m| m.iter().any(|&b| b != 0))
            } else {
                None
            };
            Some(Sock::Link {
                index: u16_at(sa, offset_of!(ffi::SockaddrDl, index))?,
                ftype: *sa.get(offset_of!(ffi::SockaddrDl, ftype))?,
                mac,
            })
        }
        _ => None,
    }
}

/// The KAME stack keeps a link-local address's interface index in its second
/// 16-bit word (`fe80:4::1`); a real link-local address has zeros there
/// (RFC 4291), so clearing them gives the address as configured.
fn clear_embedded_scope(mut a: [u8; 16]) -> [u8; 16] {
    if a[0] == 0xfe && a[1] & 0xc0 == 0x80 {
        a[2] = 0;
        a[3] = 0;
    }
    a
}

/// The prefix length of a netmask's bytes for an address of `family`: the
/// leading one bits. A trimmed mask's missing bytes are zero.
fn prefix_len(mask: &[u8], family: u8) -> u8 {
    let (start, bytes) = if family == ffi::AF_INET {
        (offset_of!(ffi::SockaddrIn, addr), 4)
    } else {
        (offset_of!(ffi::SockaddrIn6, addr), 16)
    };
    let mut bits = 0u32;
    for i in 0..bytes {
        let byte = mask.get(start + i).copied().unwrap_or(0);
        bits += byte.leading_ones();
        if byte != 0xff {
            break;
        }
    }
    bits as u8
}

/// The kind from the interface type (`net/if_types.h`). Wi-Fi reports
/// `IFT_ETHER` on macOS and is not told apart by name.
fn kind_of(ftype: u8, loopback: bool) -> InterfaceKind {
    if loopback {
        return InterfaceKind::Loopback;
    }
    match ftype {
        ffi::IFT_ETHER => InterfaceKind::Ethernet,
        ffi::IFT_IEEE80211 => InterfaceKind::Wifi,
        ffi::IFT_LOOP => InterfaceKind::Loopback,
        ffi::IFT_GIF | ffi::IFT_STF | ffi::IFT_TUNNEL | ffi::IFT_PPP => InterfaceKind::Tunnel,
        ffi::IFT_BRIDGE => InterfaceKind::Bridge,
        ffi::IFT_L2VLAN | ffi::IFT_IEEE8023ADLAG => InterfaceKind::Virtual,
        ffi::IFT_CELLULAR => InterfaceKind::Cellular,
        _ => InterfaceKind::Other,
    }
}

/// iOS answers every app with this MAC instead of the real one: no address.
fn hidden_mac(mac: [u8; 6]) -> bool {
    cfg!(target_os = "ios") && mac == [0x02, 0, 0, 0, 0, 0]
}

/// Names, flags, addresses, MACs, and kinds from `getifaddrs`; with
/// `details`, the MTU and link speed from `NET_RT_IFLIST2` (its 64-bit
/// `ifi_baudrate`: `getifaddrs` gives a 32-bit one, which caps at 4 Gb/s).
pub(crate) fn interfaces(options: &InterfaceOptions) -> Result<Vec<Interface>> {
    const WHAT: &str = "network interfaces";
    let too_large = || Error::TooLarge {
        what: WHAT,
        limit: options.max_interfaces as u64,
    };
    // Addresses per interface are few; this bounds a list that is not.
    let entry_budget = options.max_interfaces.saturating_mul(64).max(1024);
    let list = IfAddrs::new()?;
    let mut out: Vec<Interface> = Vec::new();
    let mut sa = [0u8; 256];
    let mut mask = [0u8; 256];
    for (n, entry) in list.entries().enumerate() {
        if n >= entry_budget {
            return Err(too_large());
        }
        if entry.name.is_null() {
            continue;
        }
        // SAFETY: `ifa_name` is a NUL-terminated name inside the list.
        let name = unsafe { CStr::from_ptr(entry.name) }.to_bytes();
        // The entries of one interface come together: try the last one first.
        let at = match out.last().filter(|i| i.name.as_bytes() == name) {
            Some(_) => out.len() - 1,
            None => match out.iter().position(|i| i.name.as_bytes() == name) {
                Some(at) => at,
                None => {
                    if out.len() >= options.max_interfaces {
                        return Err(too_large());
                    }
                    out.push(new_interface(name, entry.flags));
                    out.len() - 1
                }
            },
        };
        let iface = &mut out[at];
        // SAFETY: `ifa_addr` is null or an address of its own `sa_len` inside
        // the list.
        let len = unsafe { copy_sockaddr(entry.addr, &mut sa) };
        match parse_sockaddr(&sa[..len]) {
            Some(Sock::Ip(addr)) => {
                // SAFETY: as above, for `ifa_netmask`.
                let mask_len = unsafe { copy_sockaddr(entry.netmask, &mut mask) };
                let family = sa[offset_of!(ffi::Sockaddr, family)];
                iface.addresses.push(IpNet {
                    addr,
                    prefix: prefix_len(&mask[..mask_len], family),
                });
            }
            Some(Sock::Link { index, ftype, mac }) => {
                iface.index = u32::from(index);
                iface.kind = kind_of(ftype, entry.flags & ffi::IFF_LOOPBACK != 0);
                iface.mac = mac.filter(|&m| !hidden_mac(m)).map(Mac::from_bytes);
                iface.is_physical = matches!(
                    iface.kind,
                    InterfaceKind::Ethernet | InterfaceKind::Wifi | InterfaceKind::Cellular
                ) && iface.mac.is_some();
            }
            None => {}
        }
    }
    if options.details {
        add_details(WHAT, &mut out)?;
    }
    Ok(out)
}

fn new_interface(name: &[u8], flags: u32) -> Interface {
    let loopback = flags & ffi::IFF_LOOPBACK != 0;
    Interface {
        name: crate::parse::lossy(name, 64),
        index: 0,
        description: None,
        kind: if loopback {
            InterfaceKind::Loopback
        } else {
            InterfaceKind::Other
        },
        mac: None,
        addresses: Vec::new(),
        up: flags & ffi::IFF_UP != 0,
        running: flags & ffi::IFF_RUNNING != 0,
        mtu: None,
        speed_mbps: None,
        is_physical: false,
    }
}

/// The MTU and link speed of each interface, matched by index.
fn add_details(what: &'static str, list: &mut [Interface]) -> Result<()> {
    with_iflist2(what, |buf| {
        walk_iflist2(what, buf, |info| {
            if let Some(iface) = list
                .iter_mut()
                .find(|i| i.index == u32::from(info.index) && i.index != 0)
            {
                iface.mtu = Some(info.mtu);
                iface.speed_mbps = Some(info.baudrate / 1_000_000).filter(|&s| s > 0);
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `if_msghdr2` message with its `sockaddr_dl` name, as the kernel
    /// lays it out.
    fn ifinfo2(index: u16, flags: i32, name: &[u8], ibytes: u64, obytes: u64) -> Vec<u8> {
        let header = size_of::<ffi::IfMsghdr2>();
        let sdl_len = offset_of!(ffi::SockaddrDl, data) + name.len();
        let len = (header + sdl_len).next_multiple_of(4);
        let mut m = vec![0u8; len];
        let put = |m: &mut Vec<u8>, at: usize, b: &[u8]| m[at..at + b.len()].copy_from_slice(b);
        put(
            &mut m,
            offset_of!(ffi::IfMsghdr2, msglen),
            &(len as u16).to_ne_bytes(),
        );
        m[offset_of!(ffi::IfMsghdr2, version)] = ffi::RTM_VERSION;
        m[offset_of!(ffi::IfMsghdr2, mtype)] = ffi::RTM_IFINFO2;
        put(
            &mut m,
            offset_of!(ffi::IfMsghdr2, addrs),
            &ffi::RTA_IFP.to_ne_bytes(),
        );
        put(
            &mut m,
            offset_of!(ffi::IfMsghdr2, flags),
            &flags.to_ne_bytes(),
        );
        put(
            &mut m,
            offset_of!(ffi::IfMsghdr2, index),
            &index.to_ne_bytes(),
        );
        put(
            &mut m,
            offset_of!(ffi::IfMsghdr2, snd_drops),
            &3i32.to_ne_bytes(),
        );
        let data = offset_of!(ffi::IfMsghdr2, data);
        m[data + offset_of!(ffi::IfData64, ftype)] = ffi::IFT_ETHER;
        put(
            &mut m,
            data + offset_of!(ffi::IfData64, mtu),
            &1500u32.to_ne_bytes(),
        );
        put(
            &mut m,
            data + offset_of!(ffi::IfData64, baudrate),
            &10_000_000_000u64.to_ne_bytes(),
        );
        put(
            &mut m,
            data + offset_of!(ffi::IfData64, ibytes),
            &ibytes.to_ne_bytes(),
        );
        put(
            &mut m,
            data + offset_of!(ffi::IfData64, obytes),
            &obytes.to_ne_bytes(),
        );
        put(
            &mut m,
            data + offset_of!(ffi::IfData64, ipackets),
            &7u64.to_ne_bytes(),
        );
        m[header] = sdl_len as u8;
        m[header + offset_of!(ffi::SockaddrDl, family)] = ffi::AF_LINK;
        m[header + offset_of!(ffi::SockaddrDl, nlen)] = name.len() as u8;
        put(&mut m, header + offset_of!(ffi::SockaddrDl, data), name);
        m
    }

    /// An address message (`RTM_NEWADDR`), which the walker skips.
    fn newaddr() -> Vec<u8> {
        let mut m = vec![0u8; 20];
        m[..2].copy_from_slice(&20u16.to_ne_bytes());
        m[2] = ffi::RTM_VERSION;
        m[3] = 0xc;
        m
    }

    #[test]
    fn test_net_apple_walker_reads_ifinfo2_and_skips_addresses() {
        let loopback = ffi::IFF_LOOPBACK.cast_signed();
        let mut buf = ifinfo2(1, loopback, b"lo0", 500, 500);
        buf.extend(newaddr());
        buf.extend(ifinfo2(4, 0, b"en0", 5_000_000_000, 1_000));
        buf.extend(newaddr());
        let mut seen = Vec::new();
        walk_iflist2("test", &buf, |i| seen.push(i)).unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].name, Some(&b"lo0"[..]));
        assert!(seen[0].is_loopback() && !seen[1].is_loopback());
        let en0 = seen[1];
        assert_eq!((en0.index, en0.mtu, en0.ftype), (4, 1500, ffi::IFT_ETHER));
        assert_eq!(en0.baudrate, 10_000_000_000, "64-bit speed, past 4 Gb/s");
        let c = en0.counters(en0.name.unwrap());
        assert_eq!(c.interface, "en0");
        assert_eq!((c.received.raw(), c.sent.raw()), (5_000_000_000, 1_000));
        assert_eq!((c.packets_received, c.dropped_out), (7, 3));
    }

    #[test]
    fn test_net_apple_walker_rejects_bad_lengths() {
        let mut zero = newaddr();
        zero[..2].copy_from_slice(&0u16.to_ne_bytes());
        assert!(matches!(
            walk_iflist2("test", &zero, |_| {}),
            Err(Error::Malformed { .. })
        ));
        let mut long = newaddr();
        long[..2].copy_from_slice(&400u16.to_ne_bytes());
        assert!(walk_iflist2("test", &long, |_| {}).is_err());
        let truncated = ifinfo2(2, 0, b"en1", 1, 1);
        let mut short = truncated[..100].to_vec();
        short[..2].copy_from_slice(&100u16.to_ne_bytes());
        assert!(walk_iflist2("test", &short, |_| {}).is_err());
        assert!(walk_iflist2("test", &[1, 0], |_| {}).is_err());
        walk_iflist2("test", &[], |_| panic!("no messages")).unwrap();
    }

    #[test]
    fn test_net_apple_walker_other_version_skipped() {
        let mut m = ifinfo2(2, 0, b"en1", 1, 1);
        m[offset_of!(ffi::IfMsghdr2, version)] = 4;
        let mut n = 0;
        walk_iflist2("test", &m, |_| n += 1).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_net_apple_prefix_len_from_netmask_bytes() {
        let v4 = |m: [u8; 4]| {
            let mut sa = vec![16, ffi::AF_INET, 0, 0];
            sa.extend(m);
            sa
        };
        assert_eq!(prefix_len(&v4([255, 255, 255, 0]), ffi::AF_INET), 24);
        assert_eq!(prefix_len(&v4([255, 255, 240, 0]), ffi::AF_INET), 20);
        assert_eq!(prefix_len(&v4([255; 4]), ffi::AF_INET), 32);
        assert_eq!(prefix_len(&v4([0; 4]), ffi::AF_INET), 0);
        // A trimmed mask: `sa_len` 7 stops after three bytes.
        assert_eq!(prefix_len(&[7, 0, 0, 0, 255, 255, 255], ffi::AF_INET), 24);
        assert_eq!(prefix_len(&[], ffi::AF_INET), 0);
        let mut v6 = vec![0u8; 28];
        v6[8..16].fill(0xff);
        assert_eq!(prefix_len(&v6, ffi::AF_INET6), 64);
        v6[8..24].fill(0xff);
        assert_eq!(prefix_len(&v6, ffi::AF_INET6), 128);
    }

    #[test]
    fn test_net_apple_parse_sockaddr_families() {
        let mut v4 = vec![16, ffi::AF_INET, 0, 0, 192, 168, 1, 20];
        v4.resize(16, 0);
        assert_eq!(
            parse_sockaddr(&v4),
            Some(Sock::Ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))))
        );
        // A link-local address with the KAME scope (interface 4) embedded.
        let mut v6 = vec![28, ffi::AF_INET6, 0, 0, 0, 0, 0, 0];
        v6.extend([0xfe, 0x80, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        v6.resize(28, 0);
        assert_eq!(
            parse_sockaddr(&v6),
            Some(Sock::Ip(IpAddr::V6(Ipv6Addr::new(
                0xfe80, 0, 0, 0, 0, 0, 0, 1
            ))))
        );
        // `en0` with a MAC after the three name bytes.
        let mut dl = vec![0u8, ffi::AF_LINK];
        dl.extend(4u16.to_ne_bytes());
        dl.extend([ffi::IFT_ETHER, 3, 6, 0]);
        dl.extend(b"en0");
        dl.extend([0x00, 0x1b, 0x21, 0xaa, 0xbb, 0x0c]);
        dl[0] = dl.len() as u8;
        assert_eq!(
            parse_sockaddr(&dl),
            Some(Sock::Link {
                index: 4,
                ftype: ffi::IFT_ETHER,
                mac: Some([0x00, 0x1b, 0x21, 0xaa, 0xbb, 0x0c]),
            })
        );
        // `lo0`: no hardware address.
        let mut lo = vec![0u8, ffi::AF_LINK];
        lo.extend(1u16.to_ne_bytes());
        lo.extend([ffi::IFT_LOOP, 3, 0, 0]);
        lo.extend(b"lo0");
        lo[0] = lo.len() as u8;
        assert_eq!(
            parse_sockaddr(&lo),
            Some(Sock::Link {
                index: 1,
                ftype: ffi::IFT_LOOP,
                mac: None
            })
        );
        assert_eq!(link_name(&lo), Some(&b"lo0"[..]));
        assert_eq!(parse_sockaddr(&[16, 99]), None);
        assert_eq!(parse_sockaddr(&[16, ffi::AF_INET, 0]), None);
        assert_eq!(parse_sockaddr(&[]), None);
    }

    #[test]
    fn test_net_apple_kind_by_interface_type() {
        assert_eq!(kind_of(ffi::IFT_ETHER, false), InterfaceKind::Ethernet);
        assert_eq!(kind_of(ffi::IFT_ETHER, true), InterfaceKind::Loopback);
        assert_eq!(kind_of(ffi::IFT_LOOP, false), InterfaceKind::Loopback);
        assert_eq!(kind_of(ffi::IFT_BRIDGE, false), InterfaceKind::Bridge);
        assert_eq!(kind_of(ffi::IFT_GIF, false), InterfaceKind::Tunnel);
        assert_eq!(kind_of(ffi::IFT_CELLULAR, false), InterfaceKind::Cellular);
        assert_eq!(kind_of(0x01, false), InterfaceKind::Other);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_net_apple_live_readings() {
        let list = interfaces(&InterfaceOptions::default()).unwrap();
        let lo0 = list.iter().find(|i| i.name == "lo0").unwrap();
        assert_eq!(lo0.kind, InterfaceKind::Loopback);
        assert!(lo0.index > 0 && lo0.mac.is_none() && lo0.mtu.is_some());
        assert!(
            lo0.addresses
                .iter()
                .any(|a| a.addr == IpAddr::V4(Ipv4Addr::LOCALHOST) && a.prefix == 8)
        );
        let tight = InterfaceOptions {
            max_interfaces: 0,
            ..InterfaceOptions::default()
        };
        assert!(matches!(interfaces(&tight), Err(Error::TooLarge { .. })));
        let counters = net_counters().unwrap();
        assert!(counters.iter().any(|c| c.interface == "lo0"));
        let before = net_totals().unwrap();
        let after = net_totals().unwrap();
        assert!(after.received >= before.received && after.sent >= before.sent);
        let c = connections().unwrap();
        assert!(c.tcp_total.is_some() && c.udp.is_some());
        assert_eq!((c.tcp_established, c.tcp_time_wait), (None, None));
    }
}
