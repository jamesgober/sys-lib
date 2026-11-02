//! Network: `getifaddrs` for interfaces and addresses, the `NET_RT_IFLIST2`
//! routing sysctl for 64-bit counters and link details, `NET_RT_DUMP` for the
//! default route, the PCB counts, and the resolver configuration.
//!
//! Every byte the OS hands back is copied out and parsed by safe code over
//! slices: the routing-message walkers and the `sockaddr` readers below.

use core::{
    ffi::{CStr, c_int, c_void},
    mem::{offset_of, size_of},
};
use std::{
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use super::{bytes_at, ffi, i32_at, last_errno, os_error, u16_at, u32_at, u64_at};
use crate::{
    Bytes, Error, Result,
    net::{
        Connections, Counters, Dns, Interface, InterfaceKind, InterfaceOptions, IpNet, Mac, Totals,
    },
    parse::resolv,
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
const RT_DUMP: &str = "sysctl NET_RT_DUMP";

/// `sysctl(CTL_NET, PF_ROUTE, 0, family, operation, 0)` into `buf`, or the size
/// it needs with no buffer: the bytes, or the `errno`.
fn route_sysctl(
    family: c_int,
    operation: c_int,
    buf: Option<&mut [u8]>,
) -> core::result::Result<usize, i32> {
    let mut mib = [ffi::CTL_NET, ffi::PF_ROUTE, 0, family, operation, 0];
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

/// Calls `f` with the `NET_RT_IFLIST2` messages.
fn with_iflist2<R>(what: &'static str, f: impl FnOnce(&[u8]) -> Result<R>) -> Result<R> {
    with_route_table(what, IFLIST2, 0, ffi::NET_RT_IFLIST2, f)
}

/// Calls `f` with a routing table's messages: in a stack buffer when they fit,
/// else in a heap buffer bounded at 16 MiB. Entries appearing between the size
/// query and the read (`ENOMEM`) mean asking again, at most three times.
fn with_route_table<R>(
    what: &'static str,
    call: &'static str,
    family: c_int,
    operation: c_int,
    f: impl FnOnce(&[u8]) -> Result<R>,
) -> Result<R> {
    let mut stack = [0u8; STACK_LIST];
    let mut heap: Vec<u8> = Vec::new();
    for _ in 0..3 {
        let need =
            route_sysctl(family, operation, None).map_err(|code| os_error(what, call, code))?;
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
        match route_sysctl(family, operation, Some(buf)) {
            Ok(len) => return f(&buf[..len.min(buf.len())]),
            Err(ffi::ENOMEM) => {}
            Err(code) => return Err(os_error(what, call, code)),
        }
    }
    Err(Error::Os {
        what,
        call,
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

/// One `sockaddr` of a routing message and what follows it. Each takes `sa_len`
/// rounded up to 4 bytes, and an empty one still takes 4 (`ROUNDUP32` in xnu's
/// `rtsock.c`).
fn next_sockaddr(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = usize::from(*data.first()?);
    let step = len.next_multiple_of(4).max(4);
    Some((data.get(..len)?, data.get(step..)?))
}

/// The interface of a route that is the machine's default: up, not a host
/// route, not bound to one interface (`RTF_IFSCOPE`: each interface keeps its
/// own such copy), to the unspecified address with an empty netmask. The
/// netmask check keeps out the `0.0.0.0/1` a VPN adds.
fn default_route(msg: &[u8]) -> Option<u32> {
    if *msg.get(offset_of!(ffi::RtMsghdr, version))? != ffi::RTM_VERSION
        || *msg.get(offset_of!(ffi::RtMsghdr, mtype))? != ffi::RTM_GET
    {
        return None;
    }
    let flags = i32_at(msg, offset_of!(ffi::RtMsghdr, flags))?;
    let addrs = i32_at(msg, offset_of!(ffi::RtMsghdr, addrs))?;
    if flags & ffi::RTF_UP == 0
        || flags & (ffi::RTF_HOST | ffi::RTF_IFSCOPE) != 0
        || addrs & ffi::RTA_DST == 0
    {
        return None;
    }
    let (dst, mut rest) = next_sockaddr(msg.get(size_of::<ffi::RtMsghdr>()..)?)?;
    let Some(Sock::Ip(ip)) = parse_sockaddr(dst) else {
        return None;
    };
    if !ip.is_unspecified() {
        return None;
    }
    if addrs & ffi::RTA_GATEWAY != 0 {
        rest = next_sockaddr(rest)?.1;
    }
    if addrs & ffi::RTA_NETMASK != 0 {
        let (mask, _) = next_sockaddr(rest)?;
        if prefix_len(mask, dst[offset_of!(ffi::Sockaddr, family)]) != 0 {
            return None;
        }
    }
    let index = u16_at(msg, offset_of!(ffi::RtMsghdr, index))?;
    (index != 0).then_some(u32::from(index))
}

/// Walks a `NET_RT_DUMP` answer for the default route. Each message is at least
/// 4 bytes, so the walk is bounded by the buffer.
fn walk_routes(what: &'static str, buf: &[u8]) -> Result<Option<u32>> {
    let malformed = || Error::Malformed {
        what,
        source: RT_DUMP,
        line: None,
    };
    let mut at = 0;
    while at < buf.len() {
        let rest = &buf[at..];
        let len =
            usize::from(u16_at(rest, offset_of!(ffi::RtMsghdr, msglen)).ok_or_else(malformed)?);
        if len < 4 || len > rest.len() {
            return Err(malformed());
        }
        if let Some(index) = default_route(&rest[..len]) {
            return Ok(Some(index));
        }
        at += len;
    }
    Ok(None)
}

/// The interface of the IPv4 default route, else the IPv6 one (an IPv6-only
/// network has no other); `None` with no default route.
pub(crate) fn primary_interface() -> Result<Option<u32>> {
    const WHAT: &str = "default route";
    for family in [ffi::AF_INET, ffi::AF_INET6] {
        let index = with_route_table(
            WHAT,
            RT_DUMP,
            c_int::from(family),
            ffi::NET_RT_DUMP,
            |buf| walk_routes(WHAT, buf),
        )?;
        if index.is_some() {
            return Ok(index);
        }
    }
    Ok(None)
}

/// The most of a resolver configuration read.
const MAX_RESOLV: u64 = 1 << 20;

/// `/etc/resolv.conf`, bounded.
fn read_resolv(what: &'static str) -> Result<Vec<u8>> {
    const PATH: &str = "/etc/resolv.conf";
    let io_error = |e: std::io::Error| os_error(what, PATH, e.raw_os_error().unwrap_or(0));
    let file = std::fs::File::open(PATH).map_err(io_error)?;
    let mut data = Vec::new();
    file.take(MAX_RESOLV + 1)
        .read_to_end(&mut data)
        .map_err(io_error)?;
    if data.len() as u64 > MAX_RESOLV {
        return Err(Error::TooLarge {
            what,
            limit: MAX_RESOLV,
        });
    }
    Ok(data)
}

fn dns_from(what: &'static str, parsed: resolv::Resolv) -> Result<Dns> {
    if parsed.too_large {
        return Err(Error::TooLarge {
            what,
            limit: resolv::MAX_ENTRIES as u64,
        });
    }
    Ok(Dns {
        servers: parsed.servers,
        upstream: Vec::new(),
        search: parsed.search,
    })
}

/// Calls `f` with each string of a list that can stand as one word of a
/// `resolv.conf` line; one past the budget are read, so a longer list is still
/// `TooLarge`. White space could not have come from the system and would change
/// the line's meaning.
#[cfg(target_os = "macos")]
fn each_word(list: Option<super::iokit::Ref<'_>>, mut f: impl FnMut(&[u8])) {
    let Some(list) = list else {
        return;
    };
    for item in list.items(resolv::MAX_ENTRIES + 1) {
        let mut buf = [0u8; 256];
        if let Some(word) = item.text(&mut buf)
            && !word.is_empty()
            && !word.iter().any(u8::is_ascii_whitespace)
        {
            f(word);
        }
    }
}

/// The global resolver from SystemConfiguration's dynamic store, written out as
/// `resolv.conf` lines so that one parser and one budget apply. `None` when the
/// store gives no server.
#[cfg(target_os = "macos")]
fn dynamic_store() -> Option<Vec<u8>> {
    use super::iokit::Cf;

    const WHAT: &str = "DNS configuration";
    let name = Cf::string(WHAT, "sys-lib").ok()?;
    let key = Cf::string(WHAT, "State:/Network/Global/DNS").ok()?;
    let servers = Cf::string(WHAT, "ServerAddresses").ok()?;
    let domains = Cf::string(WHAT, "SearchDomains").ok()?;
    // SAFETY: `name` is a live `CFString`; a null allocator is the default one,
    // and the callback and context are optional. The result, if any, carries
    // one reference.
    let store = Cf::new(unsafe {
        ffi::SCDynamicStoreCreate(
            core::ptr::null(),
            name.raw(),
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    })?;
    // SAFETY: both are live objects. The result, if any, carries one reference.
    let value = Cf::new(unsafe { ffi::SCDynamicStoreCopyValue(store.raw(), key.raw()) })?;
    let value = value.get();
    let mut text = Vec::new();
    each_word(value.value(&servers), |server| {
        text.extend_from_slice(b"nameserver ");
        text.extend_from_slice(server);
        text.push(b'\n');
    });
    if text.is_empty() {
        return None;
    }
    text.extend_from_slice(b"search");
    each_word(value.value(&domains), |domain| {
        text.push(b' ');
        text.extend_from_slice(domain);
    });
    Some(text)
}

/// iOS has no dynamic store.
#[cfg(not(target_os = "macos"))]
fn dynamic_store() -> Option<Vec<u8>> {
    None
}

/// The dynamic store on macOS, which `/etc/resolv.conf` is written from; that
/// file where the store gives nothing, and on iOS.
pub(crate) fn dns() -> Result<Dns> {
    const WHAT: &str = "DNS configuration";
    let text = match dynamic_store() {
        Some(text) => text,
        None => read_resolv(WHAT)?,
    };
    dns_from(WHAT, resolv::parse(&text))
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

    /// A `NET_RT_DUMP` message: `sockaddr`s as the kernel lays them out, each
    /// rounded up to 4 bytes; `&[0]` is an empty one, which takes 4.
    fn route(index: u16, flags: c_int, addrs: c_int, sockaddrs: &[&[u8]]) -> Vec<u8> {
        let mut m = vec![0u8; size_of::<ffi::RtMsghdr>()];
        m[offset_of!(ffi::RtMsghdr, version)] = ffi::RTM_VERSION;
        m[offset_of!(ffi::RtMsghdr, mtype)] = ffi::RTM_GET;
        let at = offset_of!(ffi::RtMsghdr, index);
        m[at..at + 2].copy_from_slice(&index.to_ne_bytes());
        let at = offset_of!(ffi::RtMsghdr, flags);
        m[at..at + 4].copy_from_slice(&flags.to_ne_bytes());
        let at = offset_of!(ffi::RtMsghdr, addrs);
        m[at..at + 4].copy_from_slice(&addrs.to_ne_bytes());
        for sa in sockaddrs {
            m.extend_from_slice(sa);
            m.resize(m.len().next_multiple_of(4), 0);
        }
        let len = m.len() as u16;
        m[..2].copy_from_slice(&len.to_ne_bytes());
        m
    }

    fn v4(a: [u8; 4]) -> Vec<u8> {
        let mut sa = vec![16, ffi::AF_INET, 0, 0];
        sa.extend(a);
        sa.resize(16, 0);
        sa
    }

    const ADDRS: c_int = ffi::RTA_DST | ffi::RTA_GATEWAY | ffi::RTA_NETMASK;

    /// `default via 192.168.1.1`: the netmask's `sa_len` is 0.
    fn default_v4(index: u16, flags: c_int) -> Vec<u8> {
        route(
            index,
            flags,
            ADDRS,
            &[&v4([0; 4]), &v4([192, 168, 1, 1]), &[0]],
        )
    }

    #[test]
    fn test_net_apple_sockaddr_stepping() {
        let (sa, rest) = next_sockaddr(&[6, 2, 0, 0, 9, 9, 0, 0, 7]).unwrap();
        assert_eq!((sa, rest), (&[6, 2, 0, 0, 9, 9][..], &[7][..]));
        // An empty one still takes four bytes.
        let (sa, rest) = next_sockaddr(&[0, 0, 0, 0, 7]).unwrap();
        assert_eq!((sa, rest), (&[][..], &[7][..]));
        assert_eq!(next_sockaddr(&[]), None);
        assert_eq!(next_sockaddr(&[16, 2]), None);
        assert_eq!(next_sockaddr(&[6, 2, 0, 0, 9, 9]), None);
    }

    #[test]
    fn test_net_apple_default_route_found() {
        assert_eq!(default_route(&default_v4(4, ffi::RTF_UP)), Some(4));
        // A directly attached default, as a point-to-point link has.
        let direct = route(
            9,
            ffi::RTF_UP,
            ffi::RTA_DST | ffi::RTA_NETMASK,
            &[&v4([0; 4]), &[0]],
        );
        assert_eq!(default_route(&direct), Some(9));
        // The netmask may also be present and all zero.
        let zero = route(
            5,
            ffi::RTF_UP,
            ADDRS,
            &[&v4([0; 4]), &v4([10, 0, 0, 1]), &[8, 0, 0, 0, 0, 0, 0, 0]],
        );
        assert_eq!(default_route(&zero), Some(5));
        let mut v6 = vec![28, ffi::AF_INET6];
        v6.resize(28, 0);
        let ipv6 = route(
            6,
            ffi::RTF_UP,
            ffi::RTA_DST | ffi::RTA_NETMASK,
            &[&v6, &[0]],
        );
        assert_eq!(default_route(&ipv6), Some(6));
    }

    #[test]
    fn test_net_apple_other_routes_are_not_default() {
        // Scoped copy, a host route, a route that is down, interface 0.
        let scoped = default_v4(4, ffi::RTF_UP | ffi::RTF_IFSCOPE);
        assert_eq!(default_route(&scoped), None);
        assert_eq!(
            default_route(&default_v4(4, ffi::RTF_UP | ffi::RTF_HOST)),
            None
        );
        assert_eq!(default_route(&default_v4(4, 0)), None);
        assert_eq!(default_route(&default_v4(0, ffi::RTF_UP)), None);
        // A network route, and a VPN's `0.0.0.0/1` with its trimmed netmask.
        let net = route(
            4,
            ffi::RTF_UP,
            ADDRS,
            &[&v4([10, 0, 0, 0]), &v4([10, 0, 0, 1]), &[0]],
        );
        assert_eq!(default_route(&net), None);
        let half = [5, 0, 0, 0, 0x80];
        let vpn = route(
            8,
            ffi::RTF_UP,
            ADDRS,
            &[&v4([0; 4]), &v4([10, 8, 0, 1]), &half],
        );
        assert_eq!(default_route(&vpn), None);
        // Another message type or version.
        let mut other = default_v4(4, ffi::RTF_UP);
        other[offset_of!(ffi::RtMsghdr, mtype)] = ffi::RTM_IFINFO2;
        assert_eq!(default_route(&other), None);
        other = default_v4(4, ffi::RTF_UP);
        other[offset_of!(ffi::RtMsghdr, version)] = 4;
        assert_eq!(default_route(&other), None);
        // Cut short.
        let full = default_v4(4, ffi::RTF_UP);
        for cut in [0, 2, 40, size_of::<ffi::RtMsghdr>(), full.len() - 8] {
            assert_eq!(default_route(&full[..cut]), None, "{cut}");
        }
    }

    #[test]
    fn test_net_apple_walks_to_the_unscoped_default() {
        let mut buf = route(
            1,
            ffi::RTF_UP,
            ADDRS,
            &[&v4([127, 0, 0, 0]), &v4([127, 0, 0, 1]), &[0]],
        );
        buf.extend(default_v4(7, ffi::RTF_UP | ffi::RTF_IFSCOPE));
        buf.extend(default_v4(4, ffi::RTF_UP));
        buf.extend(default_v4(9, ffi::RTF_UP));
        assert_eq!(walk_routes("test", &buf).unwrap(), Some(4));
        assert_eq!(walk_routes("test", &[]).unwrap(), None);
        let none = default_v4(7, ffi::RTF_UP | ffi::RTF_IFSCOPE);
        assert_eq!(walk_routes("test", &none).unwrap(), None);
        // A length that goes nowhere or past the end.
        let mut bad = default_v4(4, 0);
        bad[..2].copy_from_slice(&0u16.to_ne_bytes());
        assert!(matches!(
            walk_routes("test", &bad),
            Err(Error::Malformed { .. })
        ));
        bad[..2].copy_from_slice(&4000u16.to_ne_bytes());
        assert!(walk_routes("test", &bad).is_err());
        assert!(walk_routes("test", &[1]).is_err());
    }

    #[test]
    fn test_net_apple_dns_budget() {
        let dns = dns_from(
            "test",
            resolv::parse(b"nameserver 1.1.1.1\nsearch a.example\n"),
        );
        let dns = dns.unwrap();
        assert_eq!(dns.servers.len(), 1);
        assert_eq!(dns.search, ["a.example"]);
        assert!(dns.upstream.is_empty());
        let mut text = String::new();
        for i in 0..=resolv::MAX_ENTRIES {
            text.push_str(&format!("nameserver 10.0.0.{i}\n"));
        }
        assert!(matches!(
            dns_from("test", resolv::parse(text.as_bytes())),
            Err(Error::TooLarge { .. })
        ));
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
        // The default route, when there is one, names a listed interface.
        if let Some(index) = primary_interface().unwrap() {
            assert!(list.iter().any(|i| i.index == index), "{index}");
        }
    }
}
