//! The live readings on Linux (0.4): the file-tree logic in `crate::linux`
//! applied to the real `/`, plus the libc calls no file answers (`statvfs`,
//! `getrlimit`, `getrusage`, `clock_gettime`, `getifaddrs`, and
//! `opendir`/`readdir` to count directory entries without the heap).

use core::time::Duration;
use std::{
    ffi::CStr,
    mem::MaybeUninit,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    sync::OnceLock,
    time::SystemTime,
};

use super::{ROOT, ffi};
use crate::{
    Bytes, Error, Mhz, Pressure, Result,
    cpu::{CpuQuota, Frequency, Load, States, Times},
    disk::{Disk, DiskRaw, ListOptions, Space},
    host::{OpenFiles, Processes},
    linux::{self as tree, cgroup::Cgroups, mem::MemPaths, stat::Resource},
    mem::{HugePages, MemRaw, MemoryEvents, PagingRaw},
    net::{Connections, Counters, Interface, InterfaceKind, InterfaceOptions, IpNet, Mac, Totals},
    process::{CpuTime, Limit, Limits, ProcessIo, ProcessMemory},
};

/// The most processes counted in `/proc` (T7).
const MAX_PROCESSES: u64 = 4_000_000;

/// This process's cgroups, found once (D-045).
fn cgroups() -> &'static Cgroups {
    static CGROUPS: OnceLock<Cgroups> = OnceLock::new();
    CGROUPS.get_or_init(|| Cgroups::resolve(&ROOT))
}

/// The cgroup memory files, as paths built once, so a reading formats nothing.
fn mem_paths() -> Option<&'static MemPaths> {
    static PATHS: OnceLock<Option<MemPaths>> = OnceLock::new();
    PATHS.get_or_init(|| MemPaths::new(cgroups())).as_ref()
}

/// `USER_HZ`, the unit of `/proc/stat` (normally 100).
fn clk_tck() -> u64 {
    static TCK: OnceLock<u64> = OnceLock::new();
    *TCK.get_or_init(|| {
        u64::try_from(ffi::sysconf(ffi::SC_CLK_TCK))
            .ok()
            .filter(|&t| t > 0)
            .unwrap_or(100)
    })
}

fn last_error(what: &'static str, call: &'static str) -> Error {
    Error::Os {
        what,
        call,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

pub(crate) fn memory(with_swap: bool) -> Result<MemRaw> {
    tree::mem::read(&ROOT, mem_paths(), cgroups().lxcfs.meminfo, with_swap)
}

pub(crate) fn page_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        usize::try_from(ffi::sysconf(ffi::SC_PAGESIZE))
            .ok()
            .filter(|&s| s >= 4096)
            .unwrap_or(4096)
    })
}

pub(crate) fn mem_pressure() -> Result<Option<Pressure>> {
    tree::stat::pressure(&ROOT, cgroups(), Resource::Memory)
}

pub(crate) fn mem_events() -> Result<Option<MemoryEvents>> {
    tree::mem::events(&ROOT, mem_paths())
}

pub(crate) fn paging() -> Result<PagingRaw> {
    tree::mem::paging(&ROOT, page_size() as u64)
}

pub(crate) fn huge_pages() -> Result<Option<HugePages>> {
    tree::mem::huge_pages(&ROOT)
}

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

/// One `USER_HZ` tick: the shortest interval `/proc/stat` resolves (E-21).
pub(crate) const CPU_TICK: Duration = Duration::from_millis(10);

pub(crate) const CPU_STATES: States = States {
    iowait: true,
    steal: true,
    irq: true,
};

pub(crate) fn cpu_online(logical: usize) -> usize {
    tree::cpu::online(&ROOT).unwrap_or(logical)
}

pub(crate) fn cpu_quota() -> Result<Option<CpuQuota>> {
    Ok(tree::cpu::cpu_quota(&ROOT, cgroups()))
}

pub(crate) fn frequency() -> Frequency {
    tree::cpu::frequency(&ROOT)
}

pub(crate) fn current_frequency() -> Result<Option<Mhz>> {
    tree::cpu::current_frequency(&ROOT)
}

pub(crate) fn load() -> Result<Option<Load>> {
    tree::stat::load(&ROOT)
}

pub(crate) fn cpu_pressure() -> Result<Option<Pressure>> {
    tree::stat::pressure(&ROOT, cgroups(), Resource::Cpu)
}

pub(crate) fn cpu_times() -> Result<Times> {
    tree::stat::cpu_times(&ROOT, clk_tck())
}

pub(crate) fn cpu_times_per_core(out: &mut [Times]) -> Result<(Times, usize)> {
    tree::stat::cpu_times_per_core(&ROOT, clk_tck(), out)
}

/// The cgroup-aware count of CPUs this process may use.
pub(super) fn available(logical: usize) -> usize {
    tree::cpu::available(&ROOT, cgroups(), logical)
}

// ---------------------------------------------------------------------------
// Disks
// ---------------------------------------------------------------------------

/// `path` as a NUL-terminated string on the stack (no heap for paths under
/// 4 KiB), handed to `f`.
fn with_c_path<T>(path: &Path, f: impl FnOnce(&CStr) -> Result<T>) -> Result<T> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let invalid = Error::InvalidInput {
        what: "path",
        reason: "must not contain a NUL byte",
    };
    let mut buf = [0u8; 4096];
    if bytes.len() < buf.len() {
        buf[..bytes.len()].copy_from_slice(bytes);
        let c = CStr::from_bytes_until_nul(&buf[..=bytes.len()]).map_err(|_| invalid)?;
        if c.to_bytes().len() != bytes.len() {
            return Err(Error::InvalidInput {
                what: "path",
                reason: "must not contain a NUL byte",
            });
        }
        f(c)
    } else {
        let owned = std::ffi::CString::new(bytes).map_err(|_| invalid)?;
        f(&owned)
    }
}

fn statvfs(path: &Path) -> Result<ffi::Statvfs> {
    with_c_path(path, |c| {
        let mut buf = MaybeUninit::<ffi::Statvfs>::zeroed();
        // SAFETY: `c` is NUL-terminated and `buf` is a writable `statvfs`.
        let rc = unsafe { ffi::statvfs(c.as_ptr(), buf.as_mut_ptr()) };
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            return Err(Error::io("disk space", "statvfs", e));
        }
        // SAFETY: `statvfs` succeeded, so it initialised `buf`.
        Ok(unsafe { buf.assume_init() })
    })
}

fn space_of(st: &ffi::Statvfs) -> Space {
    let unit = st.frsize;
    Space::new(
        st.blocks.saturating_mul(unit),
        st.bfree.saturating_mul(unit),
        st.bavail.saturating_mul(unit),
    )
}

pub(crate) fn disk_space(path: &Path) -> Result<Space> {
    statvfs(path).map(|st| space_of(&st))
}

pub(crate) fn disks(options: &ListOptions) -> Result<Vec<Disk>> {
    let mounts = tree::disk::mounts(&ROOT, options)?;
    Ok(mounts
        .into_iter()
        .map(|m| {
            let st = statvfs(&m.mount_point).ok();
            let read_only =
                m.read_only || st.as_ref().is_some_and(|s| s.flag & ffi::ST_RDONLY != 0);
            Disk {
                space: st.as_ref().map(space_of),
                mount_point: m.mount_point,
                device: m.device,
                label: None,
                filesystem: m.filesystem,
                kind: m.kind,
                read_only,
                removable: m.removable,
            }
        })
        .collect())
}

pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    tree::disk::io_totals(&ROOT)
}

pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    tree::disk::io_counters(&ROOT)
}

pub(crate) fn disk_pressure() -> Result<Option<Pressure>> {
    tree::stat::pressure(&ROOT, cgroups(), Resource::Io)
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// The `getifaddrs` list, freed when dropped.
struct IfAddrs(*mut ffi::Ifaddrs);

impl Drop for IfAddrs {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a successful `getifaddrs` and is
            // freed exactly once, here.
            unsafe { ffi::freeifaddrs(self.0) };
        }
    }
}

/// The prefix length of a netmask: its leading one bits.
fn prefix_len(mask: &[u8]) -> u8 {
    let mut n = 0u32;
    for &b in mask {
        n += b.leading_ones();
        if b != 0xff {
            break;
        }
    }
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// The address in a `sockaddr` and its prefix length from the netmask.
///
/// # Safety
///
/// `addr` and `mask` are null or point to `sockaddr` structures of the family
/// their first field says, as `getifaddrs` returns them.
unsafe fn address(addr: *const ffi::Sockaddr, mask: *const ffi::Sockaddr) -> Option<IpNet> {
    if addr.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a valid `sockaddr`; the family is its first field.
    let family = unsafe { (*addr).family };
    match family {
        ffi::AF_INET => {
            // SAFETY: an AF_INET entry is a `sockaddr_in`; read unaligned, as
            // the list's buffer only guarantees `sockaddr`'s alignment.
            let a = unsafe { core::ptr::read_unaligned(addr.cast::<ffi::SockaddrIn>()) };
            let prefix = if mask.is_null() {
                32
            } else {
                // SAFETY: the netmask of an AF_INET entry is a `sockaddr_in`.
                let m = unsafe { core::ptr::read_unaligned(mask.cast::<ffi::SockaddrIn>()) };
                prefix_len(&m.addr.to_ne_bytes())
            };
            Some(IpNet {
                addr: IpAddr::V4(Ipv4Addr::from(a.addr.to_ne_bytes())),
                prefix,
            })
        }
        ffi::AF_INET6 => {
            // SAFETY: an AF_INET6 entry is a `sockaddr_in6`, read unaligned.
            let a = unsafe { core::ptr::read_unaligned(addr.cast::<ffi::SockaddrIn6>()) };
            let prefix = if mask.is_null() {
                128
            } else {
                // SAFETY: the netmask of an AF_INET6 entry is a `sockaddr_in6`.
                let m = unsafe { core::ptr::read_unaligned(mask.cast::<ffi::SockaddrIn6>()) };
                prefix_len(&m.addr)
            };
            Some(IpNet {
                addr: IpAddr::V6(Ipv6Addr::from(a.addr)),
                prefix,
            })
        }
        _ => None,
    }
}

pub(crate) fn interfaces(options: &InterfaceOptions) -> Result<Vec<Interface>> {
    let mut head: *mut ffi::Ifaddrs = std::ptr::null_mut();
    // SAFETY: `head` is a writable pointer the call fills.
    if unsafe { ffi::getifaddrs(&raw mut head) } != 0 {
        return Err(last_error("network interfaces", "getifaddrs"));
    }
    let list = IfAddrs(head);
    let mut out: Vec<(Interface, Option<u16>)> = Vec::new();
    let mut cursor = list.0;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a node of the live list `list` owns.
        let entry = unsafe { &*cursor };
        cursor = entry.next;
        if entry.name.is_null() {
            continue;
        }
        // SAFETY: `ifa_name` is a NUL-terminated string inside the list.
        let raw_name = unsafe { CStr::from_ptr(entry.name) }.to_bytes();
        let i = match out.iter().position(|(i, _)| i.name.as_bytes() == raw_name) {
            Some(i) => i,
            None => {
                if out.len() >= options.max_interfaces {
                    return Err(Error::TooLarge {
                        what: "network interfaces",
                        limit: options.max_interfaces as u64,
                    });
                }
                let up = entry.flags & ffi::IFF_UP != 0;
                let loopback = entry.flags & ffi::IFF_LOOPBACK != 0;
                out.push((
                    Interface {
                        name: crate::parse::lossy(raw_name, 64),
                        index: 0,
                        description: None,
                        kind: if loopback {
                            InterfaceKind::Loopback
                        } else {
                            InterfaceKind::Other
                        },
                        mac: None,
                        addresses: Vec::new(),
                        up,
                        running: entry.flags & ffi::IFF_RUNNING != 0,
                        mtu: None,
                        speed_mbps: None,
                        is_physical: false,
                    },
                    None,
                ));
                out.len() - 1
            }
        };
        if entry.addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_addr` is a valid `sockaddr`; the family is its first field.
        let family = unsafe { (*entry.addr).family };
        if family == ffi::AF_PACKET {
            // SAFETY: an AF_PACKET entry's address is a `sockaddr_ll`, read
            // unaligned.
            let ll = unsafe { core::ptr::read_unaligned(entry.addr.cast::<ffi::SockaddrLl>()) };
            let (iface, hatype) = &mut out[i];
            iface.index = u32::try_from(ll.ifindex).unwrap_or(0);
            *hatype = Some(ll.hatype);
            if ll.halen == 6 {
                let mut bytes = [0u8; 6];
                bytes.copy_from_slice(&ll.addr[..6]);
                if bytes != [0; 6] {
                    iface.mac = Some(Mac::from_bytes(bytes));
                }
            }
        } else {
            // SAFETY: both pointers come from the same `getifaddrs` entry.
            if let Some(net) = unsafe { address(entry.addr, entry.netmask) } {
                let iface = &mut out[i].0;
                if iface.addresses.len() < 256 && !iface.addresses.contains(&net) {
                    iface.addresses.push(net);
                }
            }
        }
    }
    drop(list);
    Ok(out
        .into_iter()
        .map(|(mut iface, hatype)| {
            let name = iface.name.clone();
            if options.details {
                let d = tree::net::details(&ROOT, name.as_bytes(), hatype);
                if let Some(kind) = d.kind {
                    iface.kind = kind;
                }
                if let (Some(mac), Some(kind)) = (iface.mac.as_mut(), d.mac_kind) {
                    mac.kind = kind;
                }
                if let Some(running) = d.running {
                    iface.running = running;
                }
                iface.mtu = d.mtu;
                iface.speed_mbps = d.speed_mbps;
                iface.is_physical = d.is_physical;
            } else if let Some(h) = hatype
                && iface.kind != InterfaceKind::Loopback
            {
                iface.kind = tree::net::kind_from(h, false, false, false);
            }
            iface
        })
        .collect())
}

pub(crate) fn net_totals() -> Result<Totals> {
    tree::net::totals(&ROOT)
}

pub(crate) fn net_counters() -> Result<Vec<Counters>> {
    tree::net::counters(&ROOT, 4096)
}

pub(crate) fn connections() -> Result<Connections> {
    tree::net::connections(&ROOT)
}

// ---------------------------------------------------------------------------
// This process
// ---------------------------------------------------------------------------

/// A directory stream, closed when dropped.
struct DirStream(*mut ffi::Dir);

impl Drop for DirStream {
    fn drop(&mut self) {
        // SAFETY: the stream came from a successful `opendir` and is closed
        // exactly once, here.
        let _ = unsafe { ffi::closedir(self.0) };
    }
}

/// Counts the entries of `dir` (without `.` and `..`) for which `keep` is
/// true, up to `max`, through `readdir`: no heap, whatever the count.
fn count_entries(dir: &CStr, max: u64, keep: impl Fn(&[u8]) -> bool) -> Result<u64> {
    // SAFETY: `dir` is NUL-terminated.
    let stream = unsafe { ffi::opendir(dir.as_ptr()) };
    if stream.is_null() {
        return Err(Error::io(
            "directory count",
            "opendir",
            std::io::Error::last_os_error(),
        ));
    }
    let stream = DirStream(stream);
    let mut n = 0u64;
    loop {
        // SAFETY: `stream` is open; `readdir` returns null at the end.
        let entry = unsafe { ffi::readdir(stream.0) };
        if entry.is_null() {
            return Ok(n);
        }
        // SAFETY: a non-null entry is valid until the next `readdir` call.
        let entry = unsafe { &*entry };
        // SAFETY: the name is NUL-terminated within `d_name`.
        let name = unsafe { CStr::from_ptr(entry.name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." || !keep(name) {
            continue;
        }
        n += 1;
        if n > max {
            return Err(Error::TooLarge {
                what: "directory count",
                limit: max,
            });
        }
    }
}

pub(crate) fn parent_id() -> Option<u32> {
    tree::process::status_number(&ROOT, b"PPid")
        .ok()
        .flatten()
        .and_then(|p| u32::try_from(p).ok())
        .filter(|&p| p > 0)
}

pub(crate) fn process_memory() -> Result<Bytes> {
    tree::process::memory(&ROOT, page_size() as u64)
}

pub(crate) fn peak_memory() -> Result<Bytes> {
    tree::process::peak_memory(&ROOT)
}

pub(crate) fn memory_detail() -> Result<ProcessMemory> {
    tree::process::memory_detail(&ROOT)
}

fn duration(t: ffi::Timeval) -> Duration {
    let secs = u64::try_from(t.sec).unwrap_or(0);
    let micros = u32::try_from(t.usec).unwrap_or(0).min(999_999);
    Duration::new(secs, micros * 1000)
}

pub(crate) fn cpu_time() -> Result<CpuTime> {
    let mut usage = MaybeUninit::<ffi::Rusage>::zeroed();
    // SAFETY: `usage` is a writable `rusage`.
    if unsafe { ffi::getrusage(ffi::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(last_error("process CPU time", "getrusage"));
    }
    // SAFETY: `getrusage` succeeded, so it initialised `usage`.
    let usage = unsafe { usage.assume_init() };
    Ok(CpuTime {
        user: duration(usage.utime),
        system: duration(usage.stime),
    })
}

pub(crate) fn threads() -> Result<usize> {
    tree::process::status_number(&ROOT, b"Threads")?
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(Error::Malformed {
            what: "thread count",
            source: "/proc/self/status",
            line: None,
        })
}

pub(crate) fn open_files() -> Result<usize> {
    let n = count_entries(c"/proc/self/fd", 1 << 24, |_| true)?;
    // The directory stream's own descriptor is in the listing.
    Ok(usize::try_from(n.saturating_sub(1)).unwrap_or(usize::MAX))
}

fn rlimit(resource: core::ffi::c_int) -> Result<Limit> {
    let mut lim = ffi::Rlimit::default();
    // SAFETY: `lim` is a writable `rlimit`.
    if unsafe { ffi::getrlimit(resource, &raw mut lim) } != 0 {
        return Err(last_error("resource limits", "getrlimit"));
    }
    let value = |v: u64| (v != ffi::RLIM_INFINITY).then_some(v);
    Ok(Limit {
        soft: value(lim.cur),
        hard: value(lim.max),
    })
}

pub(crate) fn limits() -> Result<Limits> {
    Ok(Limits {
        open_files: rlimit(ffi::RLIMIT_NOFILE)?,
        processes: rlimit(ffi::RLIMIT_NPROC)?,
        address_space: rlimit(ffi::RLIMIT_AS)?,
        core_dump: rlimit(ffi::RLIMIT_CORE)?,
        stack: rlimit(ffi::RLIMIT_STACK)?,
    })
}

pub(crate) fn started_at() -> Result<SystemTime> {
    tree::process::started_at(&ROOT, clk_tck(), boot_time()?)
}

pub(crate) fn process_io() -> Result<ProcessIo> {
    tree::process::io(&ROOT)
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

pub(crate) fn uptime() -> Result<Duration> {
    let mut ts = ffi::Timespec::default();
    // SAFETY: `ts` is a writable `timespec`.
    if unsafe { ffi::clock_gettime(ffi::CLOCK_BOOTTIME, &raw mut ts) } == 0 {
        let secs = u64::try_from(ts.sec).unwrap_or(0);
        let nanos = u32::try_from(ts.nsec).unwrap_or(0).min(999_999_999);
        return Ok(Duration::new(secs, nanos));
    }
    // Kernels without CLOCK_BOOTTIME (before 2.6.39).
    tree::stat::uptime(&ROOT)
}

pub(crate) fn boot_time() -> Result<SystemTime> {
    tree::stat::boot_time(&ROOT)
}

pub(crate) fn processes() -> Result<Processes> {
    let total = count_entries(c"/proc", MAX_PROCESSES, |name| {
        name.iter().all(u8::is_ascii_digit)
    })?;
    Ok(Processes {
        total,
        threads: tree::stat::threads(&ROOT),
        running: tree::stat::procs_running(&ROOT),
    })
}

pub(crate) fn host_open_files() -> Result<OpenFiles> {
    tree::host::open_files(&ROOT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_linux_prefix_len() {
        assert_eq!(prefix_len(&[255, 255, 255, 0]), 24);
        assert_eq!(prefix_len(&[255, 255, 240, 0]), 20);
        assert_eq!(prefix_len(&[0; 4]), 0);
        assert_eq!(prefix_len(&[255; 16]), 128);
        assert_eq!(prefix_len(&[255, 128, 255, 0]), 9);
    }

    #[test]
    fn test_sys_linux_live_counts() {
        assert!(open_files().unwrap() >= 3, "stdin, stdout, stderr");
        let p = processes().unwrap();
        assert!(p.total >= 1);
        assert!(p.threads.unwrap() >= p.total);
        assert!(uptime().unwrap() > Duration::ZERO);
        let l = limits().unwrap();
        if let (Some(s), Some(h)) = (l.open_files.soft, l.open_files.hard) {
            assert!(s <= h);
        }
        assert!(matches!(
            with_c_path(Path::new("a\0b"), |_| Ok(())),
            Err(Error::InvalidInput { .. })
        ));
    }

    #[test]
    fn test_sys_linux_interfaces_have_loopback() {
        let list = interfaces(&InterfaceOptions::default()).unwrap();
        let lo = list
            .iter()
            .find(|i| i.kind == InterfaceKind::Loopback)
            .unwrap();
        assert!(lo.index > 0 && lo.up);
        assert!(lo.addresses.iter().any(|a| a.addr.is_loopback()));
    }
}
