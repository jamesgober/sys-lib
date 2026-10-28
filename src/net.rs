//! Network interfaces, download and upload, and connection counts.
//!
//! sys-lib never sends a packet, never lists connections or their addresses,
//! and never says which program owns a socket: it reports interfaces and
//! counts.
//!
//! ```
//! use sys_lib as sys;
//!
//! for i in sys::net::interfaces()? {
//!     println!("{} {:?} up={} speed={:?}", i.name, i.kind, i.running, i.speed_mbps);
//! }
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::{fmt, time::Duration};
use std::{net::IpAddr, time::Instant};

use crate::{Bytes, Error, Rate, Result};

/// One network interface.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Interface {
    /// The name: `eth0`, `en0`, `Ethernet 2` (UTF-8; converted lossily).
    pub name: String,
    /// The OS's interface index.
    pub index: u32,
    /// The friendly description (Windows).
    pub description: Option<String>,
    /// What kind of interface it is.
    pub kind: InterfaceKind,
    /// The hardware address, when it has one (E-53).
    pub mac: Option<Mac>,
    /// The addresses with their prefix lengths.
    pub addresses: Vec<IpNet>,
    /// Administratively up.
    pub up: bool,
    /// Link up, operational.
    pub running: bool,
    /// The MTU, when reported (and `details` was set).
    pub mtu: Option<u32>,
    /// The link speed in megabits per second, when the driver reports it (E-58).
    pub speed_mbps: Option<u64>,
    /// Backed by hardware, rather than a virtual or tunnel interface.
    pub is_physical: bool,
}

/// What kind of network interface.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum InterfaceKind {
    /// Wired Ethernet.
    Ethernet,
    /// Wi-Fi.
    Wifi,
    /// Loopback.
    Loopback,
    /// A tunnel (WireGuard, `tun`, GRE, IP-in-IP).
    Tunnel,
    /// A bridge.
    Bridge,
    /// Virtual (`veth`, a VM's adapter, a bond member).
    Virtual,
    /// Mobile broadband.
    Cellular,
    /// Anything else.
    Other,
}

/// A hardware (MAC) address. An identifier: for device identity use the
/// `device` module (0.5), never a raw MAC.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Mac {
    /// The six bytes.
    pub bytes: [u8; 6],
    /// Whether the address is the hardware's own or randomized.
    pub kind: MacKind,
}

impl Mac {
    /// A MAC whose kind comes from the locally administered bit, for an OS
    /// that keeps no assignment type.
    #[allow(dead_code)] // the macOS and Windows backends
    pub(crate) fn from_bytes(bytes: [u8; 6]) -> Mac {
        let kind = if bytes[0] & 0x02 != 0 {
            MacKind::Random
        } else {
            MacKind::Permanent
        };
        Mac { bytes, kind }
    }
}

impl fmt::Display for Mac {
    /// `aa:bb:cc:dd:ee:ff`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.bytes;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5]
        )
    }
}

/// Whether a MAC address is the hardware's own (E-54).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum MacKind {
    /// The hardware's burned-in address.
    Permanent,
    /// Randomized (Wi-Fi privacy addresses, Android, Windows, NetworkManager).
    Random,
    /// Not known (Linux: an address stolen from another device or set by hand).
    Unknown,
}

/// An address with its prefix length.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct IpNet {
    /// The address.
    pub addr: IpAddr,
    /// The prefix length in bits.
    pub prefix: u8,
}

impl fmt::Display for IpNet {
    /// `192.168.1.20/24`, `fe80::1/64`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// Which interfaces [`interfaces_with`] returns, and in how much detail.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct InterfaceOptions {
    /// Include interfaces that are down (default true).
    pub include_down: bool,
    /// Include loopback (default true).
    pub include_loopback: bool,
    /// Read the per-interface details: speed, MTU, kind (default true; false
    /// skips the per-interface `/sys` reads on Linux).
    pub details: bool,
    /// The most interfaces read; more is `TooLarge` (default 4 096).
    pub max_interfaces: usize,
}

impl Default for InterfaceOptions {
    fn default() -> Self {
        InterfaceOptions {
            include_down: true,
            include_loopback: true,
            details: true,
            max_interfaces: 4_096,
        }
    }
}

/// Every network interface, sorted by index, then name.
///
/// ```
/// for i in sys_lib::net::interfaces()? {
///     let mac = i.mac.map(|m| format!("{m} ({:?})", m.kind)).unwrap_or_default();
///     println!("{} {:?} up={} {mac}", i.name, i.kind, i.running);
///     for a in &i.addresses { println!("    {a}"); }
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `TooLarge` with more than 4 096 interfaces; `Os` when the OS call fails.
pub fn interfaces() -> Result<Vec<Interface>> {
    interfaces_with(InterfaceOptions::default())
}

/// The interfaces `options` selects, sorted by index, then name. A Kubernetes
/// node with thousands of `veth` interfaces lists them quickly with
/// `details: false` (E-56).
///
/// # Errors
///
/// As [`interfaces`], with `options.max_interfaces` as the budget.
pub fn interfaces_with(options: InterfaceOptions) -> Result<Vec<Interface>> {
    let mut list = crate::sys::interfaces(&options)?;
    list.retain(|i| {
        (options.include_down || i.up)
            && (options.include_loopback || i.kind != InterfaceKind::Loopback)
    });
    list.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.name.cmp(&b.name)));
    Ok(list)
}

/// The addresses of the interfaces that are up, without loopback and
/// link-local addresses (E-57).
///
/// ```
/// println!("addresses: {:?}", sys_lib::net::addresses()?);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`interfaces`].
pub fn addresses() -> Result<Vec<IpAddr>> {
    let options = InterfaceOptions {
        include_down: false,
        include_loopback: false,
        details: false,
        ..InterfaceOptions::default()
    };
    let mut out: Vec<IpAddr> = interfaces_with(options)?
        .into_iter()
        .flat_map(|i| i.addresses)
        .map(|n| n.addr)
        .filter(|a| !a.is_loopback() && !is_link_local(a))
        .collect();
    out.dedup();
    Ok(out)
}

fn is_link_local(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Bytes and packets moved since boot, every interface but loopback.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Totals {
    /// Bytes received.
    pub received: Bytes,
    /// Bytes sent.
    pub sent: Bytes,
    /// Packets received.
    pub packets_received: u64,
    /// Packets sent.
    pub packets_sent: u64,
    /// Errors, both directions.
    pub errors: u64,
    /// Packets dropped, both directions.
    pub dropped: u64,
}

/// Download and upload over an interval.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Traffic {
    /// Bytes received per second.
    pub download: Rate,
    /// Bytes sent per second.
    pub upload: Rate,
    /// Packets received per second.
    pub packets_in_per_sec: f64,
    /// Packets sent per second.
    pub packets_out_per_sec: f64,
    /// The time the values cover.
    pub interval: Duration,
}

/// One interface's traffic.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct InterfaceTraffic {
    /// The interface's name.
    pub interface: String,
    /// Its traffic.
    pub traffic: Traffic,
}

/// One interface's cumulative counters.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Counters {
    /// The interface's name.
    pub interface: String,
    /// Bytes received.
    pub received: Bytes,
    /// Bytes sent.
    pub sent: Bytes,
    /// Packets received.
    pub packets_received: u64,
    /// Packets sent.
    pub packets_sent: u64,
    /// Receive errors.
    pub errors_in: u64,
    /// Transmit errors.
    pub errors_out: u64,
    /// Received packets dropped.
    pub dropped_in: u64,
    /// Packets dropped before sending.
    pub dropped_out: u64,
}

impl Totals {
    /// Adds one interface's counters (for a backend that has only those).
    #[allow(dead_code)] // the macOS and Windows backends
    pub(crate) fn add(&mut self, c: &Counters) {
        self.received = self.received.saturating_add(c.received);
        self.sent = self.sent.saturating_add(c.sent);
        self.packets_received = self.packets_received.saturating_add(c.packets_received);
        self.packets_sent = self.packets_sent.saturating_add(c.packets_sent);
        self.errors = self
            .errors
            .saturating_add(c.errors_in)
            .saturating_add(c.errors_out);
        self.dropped = self
            .dropped
            .saturating_add(c.dropped_in)
            .saturating_add(c.dropped_out);
    }

    /// The traffic between `earlier` and `self`; a counter that went backwards
    /// (an interface re-created, a driver reset) counts as no traffic.
    fn since(&self, earlier: &Totals, interval: Duration) -> Traffic {
        let d = |new: u64, old: u64| new.checked_sub(old).unwrap_or(0);
        let secs = interval.as_secs_f64();
        let per_sec = |n: u64| if secs > 0.0 { n as f64 / secs } else { 0.0 };
        Traffic {
            download: Rate::from_delta(d(self.received.raw(), earlier.received.raw()), interval),
            upload: Rate::from_delta(d(self.sent.raw(), earlier.sent.raw()), interval),
            packets_in_per_sec: per_sec(d(self.packets_received, earlier.packets_received)),
            packets_out_per_sec: per_sec(d(self.packets_sent, earlier.packets_sent)),
            interval,
        }
    }
}

impl Counters {
    fn as_totals(&self) -> Totals {
        let mut t = Totals::default();
        t.add(self);
        t
    }
}

/// The interval [`traffic`] measures over: 1 s.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(1);

const MIN_WINDOW: Duration = Duration::from_millis(100);
const MAX_WINDOW: Duration = Duration::from_secs(60);

/// Bytes and packets moved since boot, every interface but loopback: one
/// read, no waiting. Counters are 64-bit on every platform. Inside a container
/// they are the container's network namespace.
///
/// ```
/// let total = sys_lib::net::totals()?;
/// println!("since boot: {} received, {} sent", total.received, total.sent);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` when the counters are hidden; `Malformed`; `Os`.
pub fn totals() -> Result<Totals> {
    crate::sys::net_totals()
}

/// Each interface's cumulative counters, loopback included (Tier 2).
///
/// # Errors
///
/// As [`totals`].
pub fn counters() -> Result<Vec<Counters>> {
    crate::sys::net_counters()
}

/// Download and upload over the next second. **Blocks for
/// [`DEFAULT_WINDOW`]**; a monitor keeps a [`Sampler`].
///
/// ```
/// let t = sys_lib::net::traffic()?;            // blocks 1 s
/// println!("down {} up {}", t.download, t.upload);
/// println!("down {}", t.download.bits());
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn traffic() -> Result<Traffic> {
    traffic_over(DEFAULT_WINDOW)
}

/// Download and upload over `window`, clamped to 100 ms ..= 60 s. **Blocks for
/// the window.**
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn traffic_over(window: Duration) -> Result<Traffic> {
    let mut sampler = Sampler::totals_only()?;
    std::thread::sleep(window.clamp(MIN_WINDOW, MAX_WINDOW));
    sampler.sample()
}

/// Measures download and upload between samples. Owned by the caller: two
/// samplers never disturb each other. `sample` never blocks and allocates
/// nothing.
///
/// ```no_run
/// use std::time::Duration;
///
/// let mut net = sys_lib::net::Sampler::new()?;
/// loop {
///     std::thread::sleep(Duration::from_secs(5));
///     let t = net.sample()?;
///     println!("down {} up {}", t.download, t.upload);
///     for i in net.sample_per_interface()? {
///         println!("  {}: down {} up {}", i.interface, i.traffic.download, i.traffic.upload);
///     }
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Sampler {
    total: Totals,
    at: Instant,
    interfaces: Vec<Counters>,
    interfaces_at: Instant,
}

impl Sampler {
    /// Takes the first reading, of the totals and of each interface.
    ///
    /// # Errors
    ///
    /// As [`totals`].
    pub fn new() -> Result<Sampler> {
        let now = Instant::now();
        Ok(Sampler {
            total: totals()?,
            at: now,
            interfaces: counters()?,
            interfaces_at: now,
        })
    }

    fn totals_only() -> Result<Sampler> {
        let now = Instant::now();
        Ok(Sampler {
            total: totals()?,
            at: now,
            interfaces: Vec::new(),
            interfaces_at: now,
        })
    }

    /// Download and upload since the previous sample, every interface but
    /// loopback. A counter that went backwards re-bases the sampler and reads
    /// as no traffic for that interval.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample(&mut self) -> Result<Traffic> {
        let now = totals()?;
        let at = Instant::now();
        let interval = at.duration_since(self.at);
        if interval.is_zero() {
            return Err(too_soon(interval));
        }
        let traffic = now.since(&self.total, interval);
        self.total = now;
        self.at = at;
        Ok(traffic)
    }

    /// Traffic since the previous per-interface sample, for each interface
    /// (allocates the `Vec`). An interface seen for the first time reads as no
    /// traffic.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample_per_interface(&mut self) -> Result<Vec<InterfaceTraffic>> {
        let now = counters()?;
        let at = Instant::now();
        let interval = at.duration_since(self.interfaces_at);
        if interval.is_zero() {
            return Err(too_soon(interval));
        }
        let out = now
            .iter()
            .map(|c| {
                let current = c.as_totals();
                let earlier = self
                    .interfaces
                    .iter()
                    .find(|e| e.interface == c.interface)
                    .map_or(current, Counters::as_totals);
                InterfaceTraffic {
                    interface: c.interface.clone(),
                    traffic: current.since(&earlier, interval),
                }
            })
            .collect();
        self.interfaces = now;
        self.interfaces_at = at;
        Ok(out)
    }
}

fn too_soon(elapsed: Duration) -> Error {
    Error::TooSoon {
        elapsed,
        minimum: Duration::from_millis(1),
    }
}

/// How many network connections the machine has open, as counts. A value an
/// OS does not report is `None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Connections {
    /// Open TCP connections.
    pub tcp_established: Option<u64>,
    /// Closed TCP connections waiting out the TIME_WAIT period.
    pub tcp_time_wait: Option<u64>,
    /// Every TCP socket, listening ones included.
    pub tcp_total: Option<u64>,
    /// UDP sockets.
    pub udp: Option<u64>,
}

/// How many network connections the machine has open: counts only, never
/// addresses, ports, or owners. Inside a container they are the container's
/// network namespace.
///
/// ```
/// if let Some(open) = sys_lib::net::connections()?.tcp_established {
///     println!("{open} TCP connections open");
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` when the counters are hidden; `Malformed`; `Os`.
pub fn connections() -> Result<Connections> {
    crate::sys::connections()
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn test_net_mac_display_and_kind() {
        let m = Mac::from_bytes([0x00, 0x1b, 0x21, 0xaa, 0xbb, 0x0c]);
        assert_eq!(m.to_string(), "00:1b:21:aa:bb:0c");
        assert_eq!(m.kind, MacKind::Permanent);
        assert_eq!(Mac::from_bytes([0x02, 0, 0, 0, 0, 1]).kind, MacKind::Random);
        let n = IpNet {
            addr: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
            prefix: 24,
        };
        assert_eq!(n.to_string(), "192.168.1.20/24");
        assert!(is_link_local(&IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!is_link_local(&IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn test_net_traffic_and_backwards_counters() {
        let c = |rx: u64, tx: u64, prx: u64| Counters {
            interface: "eth0".to_owned(),
            received: Bytes::new(rx),
            sent: Bytes::new(tx),
            packets_received: prx,
            packets_sent: 0,
            errors_in: 1,
            errors_out: 2,
            dropped_in: 3,
            dropped_out: 4,
        };
        let a = c(1_000_000, 0, 100).as_totals();
        let b = c(2_000_000, 500_000, 300).as_totals();
        assert_eq!((b.errors, b.dropped), (3, 7));
        let t = b.since(&a, Duration::from_secs(2));
        assert_eq!(t.download.bytes_per_sec(), 500_000);
        assert_eq!(t.upload.bytes_per_sec(), 250_000);
        assert!((t.packets_in_per_sec - 100.0).abs() < 1e-9);
        let back = a.since(&b, Duration::from_secs(1));
        assert_eq!((back.download, back.upload), (Rate::ZERO, Rate::ZERO));
    }

    #[test]
    fn test_net_live_readings() {
        let list = interfaces().unwrap();
        assert!(
            list.windows(2)
                .all(|w| (w[0].index, &w[0].name) <= (w[1].index, &w[1].name))
        );
        for i in &list {
            assert!(!i.name.is_empty());
        }
        let up = interfaces_with(InterfaceOptions {
            include_down: false,
            include_loopback: false,
            details: false,
            ..InterfaceOptions::default()
        })
        .unwrap();
        assert!(up.iter().all(|i| i.up && i.kind != InterfaceKind::Loopback));
        for a in addresses().unwrap() {
            assert!(!a.is_loopback() && !is_link_local(&a));
        }
        let t = totals().unwrap();
        let _ = (t.received, t.sent);
        let _ = connections().unwrap();
    }
}
