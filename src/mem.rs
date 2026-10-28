//! Memory: how much there is, how much is in use, and how much this process
//! may use, container-aware by default.
//!
//! Inside a container or a systemd unit with a memory limit, [`total`] is the
//! limit: what this process can use and what the out-of-memory killer
//! enforces. The host's values are one call away ([`host_total`]).
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("{} of {} used ({})", sys::mem::used()?, sys::mem::total()?, sys::mem::used_percent()?);
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::time::Duration;
use std::time::Instant;

use crate::{Bytes, Error, Percent, Pressure, Rate, Result};

/// Every memory value from one set of reads; see [`snapshot`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Memory {
    /// What this process may use: the host's memory, or the container's limit
    /// when that is smaller.
    pub total: Bytes,
    /// In use, counted against `total`.
    pub used: Bytes,
    /// Available to new allocations without swapping (reclaimable cache counts
    /// as available). `used + free == total` always holds.
    pub free: Bytes,
    /// The machine's memory, ignoring any limit.
    pub host_total: Bytes,
    /// Available on the machine, ignoring any limit.
    pub host_free: Bytes,
    /// Pages doing nothing at all (the kernel's `MemFree`), when reported.
    pub unused: Option<Bytes>,
    /// File cache that can be reclaimed, when reported.
    pub cached: Option<Bytes>,
    /// The limit applied to `total`, when there is one.
    pub limit: Option<MemoryLimit>,
    /// Swap space (all zero when there is none).
    pub swap: Swap,
    /// Where `total` and `used` came from.
    pub source: MemorySource,
}

impl Memory {
    /// `used` as a share of `total`.
    #[must_use]
    pub fn used_percent(&self) -> Percent {
        self.used.percent_of(self.total)
    }

    /// `free` as a share of `total`.
    #[must_use]
    pub fn free_percent(&self) -> Percent {
        self.free.percent_of(self.total)
    }
}

/// Where [`Memory::total`] and [`Memory::used`] came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum MemorySource {
    /// The operating system's own totals: no limit applies.
    Host,
    /// A cgroup v1 memory limit (Linux).
    Cgroup1,
    /// A cgroup v2 memory limit (Linux).
    Cgroup2,
    /// A job object's memory limit (Windows).
    JobObject,
    /// LXCFS, which already shows the container's values in `/proc/meminfo`
    /// (Linux): the limit is not applied a second time.
    Lxcfs,
}

/// A memory limit on this process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct MemoryLimit {
    /// The hard limit: the out-of-memory killer acts at this size.
    pub hard: Bytes,
    /// The throttle point (cgroup v2 `memory.high`, v1 soft limit), when set.
    pub soft: Option<Bytes>,
    /// The swap limit, when set.
    pub swap: Option<Bytes>,
    /// Where the limit came from.
    pub source: LimitSource,
}

/// Where a container or job limit came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum LimitSource {
    /// cgroup v1 (Linux).
    Cgroup1,
    /// cgroup v2 (Linux).
    Cgroup2,
    /// A job object (Windows).
    JobObject,
}

/// Swap space. All zero when there is none; never NaN in any percentage.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Swap {
    /// The size of all swap space.
    pub total: Bytes,
    /// In use.
    pub used: Bytes,
    /// Free. `used + free == total`.
    pub free: Bytes,
}

impl Swap {
    pub(crate) fn new(total: u64, free: u64) -> Swap {
        let free = free.min(total);
        Swap {
            total: Bytes::new(total),
            used: Bytes::new(total - free),
            free: Bytes::new(free),
        }
    }
}

/// Which parts of the memory reading a caller needs, so each Tier 1 function
/// performs only the reads it needs (`host_total()` reads no cgroup file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemWant {
    /// The limit (cgroup `memory.max` and its ancestors, the job object).
    pub(crate) limit: bool,
    /// The usage counted against the limit (`memory.current`, `memory.stat`).
    pub(crate) usage: bool,
    /// Swap, and on Windows the cache.
    pub(crate) swap: bool,
}

impl MemWant {
    pub(crate) const HOST: MemWant = MemWant {
        limit: false,
        usage: false,
        swap: false,
    };
    pub(crate) const TOTAL: MemWant = MemWant {
        limit: true,
        usage: false,
        swap: false,
    };
    pub(crate) const USED: MemWant = MemWant {
        limit: true,
        usage: true,
        swap: false,
    };
    pub(crate) const SWAP: MemWant = MemWant {
        limit: false,
        usage: false,
        swap: true,
    };
    pub(crate) const ALL: MemWant = MemWant {
        limit: true,
        usage: true,
        swap: true,
    };
    /// Everything, for the backend tests.
    #[cfg(test)]
    pub(crate) const ALL_FOR_TESTS: MemWant = MemWant::ALL;
}

/// Memory as a platform reports it, in bytes. [`from_raw`] turns it into the
/// public values and enforces their invariants in one place.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MemRaw {
    pub(crate) host_total: u64,
    pub(crate) host_available: u64,
    pub(crate) host_unused: Option<u64>,
    pub(crate) cached: Option<u64>,
    /// The limit, as configured (it may exceed the host's memory).
    pub(crate) limit: Option<MemoryLimit>,
    /// Usage counted against the limit (the cgroup's working set), when there
    /// is a limit.
    pub(crate) limited_used: Option<u64>,
    /// Swap (total, free), when it was asked for.
    pub(crate) swap: Option<(u64, u64)>,
    /// `Host`, `Lxcfs`, or where the limit came from.
    pub(crate) source: Option<MemorySource>,
}

/// The public memory values from a platform's raw reading.
///
/// - A limit at or above the host's memory is no limit (E-9).
/// - Inside a limit, used is the limit's own usage, at most the limit (E-11).
/// - `used + free == total`, `used <= total`, and the host's values obey the
///   same rules, whatever the platform reported (threat T5: values read at
///   different instants).
pub(crate) fn from_raw(raw: MemRaw) -> Memory {
    let host_total = raw.host_total;
    let host_free = raw.host_available.min(host_total);
    let limit = raw
        .limit
        .filter(|l| l.hard.raw() > 0 && l.hard.raw() < host_total);
    let (total, used) = match (limit, raw.limited_used) {
        (Some(l), Some(used)) => (l.hard.raw(), used.min(l.hard.raw())),
        // A limit without its own usage: the host's usage, capped at the limit.
        (Some(l), None) => (l.hard.raw(), (host_total - host_free).min(l.hard.raw())),
        (None, _) => (host_total, host_total - host_free),
    };
    let source = match (limit, raw.source) {
        (Some(l), _) => match l.source {
            LimitSource::Cgroup1 => MemorySource::Cgroup1,
            LimitSource::Cgroup2 => MemorySource::Cgroup2,
            LimitSource::JobObject => MemorySource::JobObject,
        },
        (None, Some(MemorySource::Lxcfs)) => MemorySource::Lxcfs,
        (None, _) => MemorySource::Host,
    };
    Memory {
        total: Bytes::new(total),
        used: Bytes::new(used),
        free: Bytes::new(total - used),
        host_total: Bytes::new(host_total),
        host_free: Bytes::new(host_free),
        unused: raw.host_unused.map(|u| Bytes::new(u.min(host_total))),
        cached: raw.cached.map(|c| Bytes::new(c.min(host_total))),
        limit,
        swap: raw
            .swap
            .map_or_else(Swap::default, |(t, f)| Swap::new(t, f)),
        source,
    }
}

fn read(want: MemWant) -> Result<Memory> {
    crate::sys::memory(want).map(from_raw)
}

/// What this process may use: the host's memory, or the container's (or job
/// object's) limit when that is smaller. A live reading: the limit can change
/// while a process runs (Kubernetes in-place resize, VM ballooning).
///
/// ```
/// use sys_lib as sys;
///
/// // Size a cache to a quarter of what this process may use.
/// let cache_bytes = sys::mem::total()?.raw() / 4;
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` or `PermissionDenied` when a sandbox hides the OS's memory
/// counters; `Malformed` when they do not parse.
pub fn total() -> Result<Bytes> {
    read(MemWant::TOTAL).map(|m| m.total)
}

/// Memory in use, counted against [`total`]. Inside a limit it is the
/// container's working set: current usage minus inactive file pages, the
/// definition the kubelet uses for eviction.
///
/// # Errors
///
/// As [`total`].
pub fn used() -> Result<Bytes> {
    read(MemWant::USED).map(|m| m.used)
}

/// Memory available to new allocations without swapping: reclaimable cache
/// counts as available (`MemAvailable` on Linux).
///
/// # Errors
///
/// As [`total`].
pub fn free() -> Result<Bytes> {
    read(MemWant::USED).map(|m| m.free)
}

/// [`used`] as a share of [`total`].
///
/// # Errors
///
/// As [`total`].
pub fn used_percent() -> Result<Percent> {
    read(MemWant::USED).map(|m| m.used_percent())
}

/// [`free`] as a share of [`total`].
///
/// ```
/// use sys_lib as sys;
///
/// // Shed load before the out-of-memory killer does.
/// if sys::mem::free_percent()?.value() < 5.0 { /* refuse new work */ }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`total`].
pub fn free_percent() -> Result<Percent> {
    read(MemWant::USED).map(|m| m.free_percent())
}

/// Every memory value from one set of reads, swap included. A caller who wants
/// several values calls this once.
///
/// ```
/// use sys_lib as sys;
///
/// let m = sys::mem::snapshot()?;
/// println!("{} free of {} (host {} of {}), source {:?}", m.free, m.total, m.host_free, m.host_total, m.source);
/// if let Some(limit) = m.limit {
///     println!("limit {} from {:?}, throttles at {:?}", limit.hard, limit.source, limit.soft);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`total`].
pub fn snapshot() -> Result<Memory> {
    read(MemWant::ALL)
}

/// The machine's memory, ignoring any limit.
///
/// # Errors
///
/// As [`total`].
pub fn host_total() -> Result<Bytes> {
    read(MemWant::HOST).map(|m| m.host_total)
}

/// Memory available on the machine, ignoring any limit.
///
/// # Errors
///
/// As [`total`].
pub fn host_free() -> Result<Bytes> {
    read(MemWant::HOST).map(|m| m.host_free)
}

/// The memory limit on this process, or `None` when there is none (or it is
/// at or above the machine's memory). Linux: the smallest cgroup v2
/// `memory.max` over the cgroup and its ancestors, or cgroup v1's
/// `hierarchical_memory_limit`. Windows: the job object's process or job memory
/// limit. macOS has no equivalent: `None`.
///
/// # Errors
///
/// As [`total`].
pub fn limit() -> Result<Option<MemoryLimit>> {
    read(MemWant::TOTAL).map(|m| m.limit)
}

/// Swap space. All zero when there is none (E-31). zram counts as swap; zswap
/// is invisible to these counters (E-32).
///
/// # Errors
///
/// As [`total`].
pub fn swap() -> Result<Swap> {
    read(MemWant::SWAP).map(|m| m.swap)
}

/// The size of a memory page in bytes (a static fact).
#[must_use]
pub fn page_size() -> usize {
    crate::sys::page_size()
}

/// How much work is stalled waiting for memory ([`Pressure`]). Linux: the
/// cgroup's `memory.pressure`, else `/proc/pressure/memory`; `None` on kernels
/// without pressure-stall information. macOS: the kernel's memory-pressure
/// level, with no stall shares. Windows: `Critical` when the system signals low
/// memory, else `Normal`, with no stall shares.
///
/// ```
/// use sys_lib::{self as sys, PressureLevel};
///
/// if let Some(p) = sys::mem::pressure()? {
///     if p.level >= PressureLevel::Warning { /* trim caches */ }
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Malformed` when the kernel's file does not parse.
pub fn pressure() -> Result<Option<Pressure>> {
    crate::sys::mem_pressure()
}

/// Out-of-memory events, counted since the cgroup (or the machine) started.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct MemoryEvents {
    /// Processes the out-of-memory killer ended.
    pub oom_kills: u64,
    /// Times the limit was reached and reclaim failed.
    pub oom: u64,
    /// Times usage hit the hard limit.
    pub max: u64,
    /// Times usage went over the throttle point (`memory.high`).
    pub high: u64,
    /// Whether the counts are this cgroup's or the whole machine's.
    pub scope: EventScope,
}

/// What [`MemoryEvents`] count.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum EventScope {
    /// This process's cgroup.
    Cgroup,
    /// The whole machine.
    #[default]
    Host,
}

/// Whether this container (or machine) has been running out of memory. A
/// watchdog compares two readings and sheds load before the next kill.
///
/// Linux: cgroup v2 `memory.events`, cgroup v1 `memory.oom_control`, and
/// outside a cgroup `/proc/vmstat` (`max` and `high` are then 0). macOS and
/// Windows keep no such counters: `None`.
///
/// ```
/// use sys_lib as sys;
///
/// if let Some(e) = sys::mem::events()? {
///     println!("{} out-of-memory kills ({:?})", e.oom_kills, e.scope);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Malformed` when a counter file does not parse.
pub fn events() -> Result<Option<MemoryEvents>> {
    crate::sys::mem_events()
}

/// Swap and paging activity over an interval: whether the machine is
/// thrashing.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Activity {
    /// Memory moved from swap back into RAM.
    pub swap_in: Rate,
    /// Memory moved from RAM out to swap.
    pub swap_out: Rate,
    /// Page faults that had to read from disk, per second, when reported.
    pub major_faults_per_sec: Option<f64>,
    /// The time the values cover.
    pub interval: Duration,
}

/// Cumulative paging counters as a platform reports them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct PagingRaw {
    pub(crate) swap_in: u64,
    pub(crate) swap_out: u64,
    pub(crate) major_faults: Option<u64>,
}

/// The interval [`activity`] measures over.
const ACTIVITY_WINDOW: Duration = Duration::from_secs(1);

/// Swap and paging activity over the next second. **Blocks for one second**;
/// a monitor keeps a [`Sampler`] instead.
///
/// # Errors
///
/// `Unsupported` where the OS has no system-wide paging counters; otherwise as
/// [`Sampler::sample`].
pub fn activity() -> Result<Activity> {
    let mut sampler = Sampler::new()?;
    std::thread::sleep(ACTIVITY_WINDOW);
    sampler.sample()
}

/// Measures swap and paging activity between samples. Owned by the caller: two
/// samplers never disturb each other. `sample` never blocks and allocates
/// nothing.
///
/// ```no_run
/// use std::time::Duration;
/// use sys_lib as sys;
///
/// let mut paging = sys::mem::Sampler::new()?;
/// std::thread::sleep(Duration::from_secs(5));
/// let a = paging.sample()?;
/// if a.swap_out.bytes_per_sec() > 0 && a.major_faults_per_sec.unwrap_or(0.0) > 100.0 {
///     println!("thrashing: swapping out {}", a.swap_out);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Sampler {
    previous: PagingRaw,
    at: Instant,
}

impl Sampler {
    /// Takes the first reading.
    ///
    /// # Errors
    ///
    /// `Unsupported` where the OS has no system-wide paging counters.
    pub fn new() -> Result<Sampler> {
        Ok(Sampler {
            previous: crate::sys::paging()?,
            at: Instant::now(),
        })
    }

    /// The activity since the previous sample. A counter that went backwards
    /// re-bases the sampler and counts as no activity.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample(&mut self) -> Result<Activity> {
        let now = crate::sys::paging()?;
        let at = Instant::now();
        let interval = at.duration_since(self.at);
        if interval.is_zero() {
            return Err(Error::TooSoon {
                elapsed: interval,
                minimum: Duration::from_millis(1),
            });
        }
        let delta = |new: u64, old: u64| new.saturating_sub(old);
        let activity = Activity {
            swap_in: Rate::from_delta(delta(now.swap_in, self.previous.swap_in), interval),
            swap_out: Rate::from_delta(delta(now.swap_out, self.previous.swap_out), interval),
            major_faults_per_sec: match (now.major_faults, self.previous.major_faults) {
                (Some(n), Some(o)) => Some(delta(n, o) as f64 / interval.as_secs_f64()),
                _ => None,
            },
            interval,
        };
        self.previous = now;
        self.at = at;
        Ok(activity)
    }
}

/// Explicit huge pages and the transparent-huge-page mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct HugePages {
    /// The default huge page size (2 MiB on x86_64).
    pub page_size: Bytes,
    /// Huge pages in the pool.
    pub total: u64,
    /// Free huge pages.
    pub free: u64,
    /// Reserved but not yet allocated.
    pub reserved: u64,
    /// The transparent-huge-page mode (Linux).
    pub transparent: Option<TransparentHugePages>,
    /// The smallest large page (Windows `GetLargePageMinimum`).
    pub large_page_minimum: Option<Bytes>,
}

/// The Linux transparent-huge-page mode.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum TransparentHugePages {
    /// Used for every eligible mapping.
    Always,
    /// Used only where the program asks with `madvise`.
    Madvise,
    /// Not used.
    Never,
}

/// Explicit huge pages and the transparent-huge-page mode, which databases and
/// JIT runtimes care about. Linux: `/proc/meminfo` and
/// `/sys/kernel/mm/transparent_hugepage/enabled`. Windows: the large page
/// minimum (the pool counts are zero). macOS: `None`.
///
/// # Errors
///
/// `Malformed` when the kernel's files do not parse.
pub fn huge_pages() -> Result<Option<HugePages>> {
    crate::sys::huge_pages()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(hard: u64, source: LimitSource) -> MemoryLimit {
        MemoryLimit {
            hard: Bytes::new(hard),
            soft: None,
            swap: None,
            source,
        }
    }

    const GIB: u64 = 1 << 30;

    #[test]
    fn test_mem_host_values_without_a_limit() {
        let m = from_raw(MemRaw {
            host_total: 16 * GIB,
            host_available: 10 * GIB,
            host_unused: Some(2 * GIB),
            ..MemRaw::default()
        });
        assert_eq!(
            (m.total.raw(), m.used.raw(), m.free.raw()),
            (16 * GIB, 6 * GIB, 10 * GIB)
        );
        assert_eq!(m.source, MemorySource::Host);
        assert_eq!(m.limit, None);
        assert_eq!(m.swap, Swap::default());
        assert_eq!(m.used_percent().to_string(), "37.5%");
    }

    #[test]
    fn test_mem_limit_below_host_wins_with_its_own_usage() {
        let m = from_raw(MemRaw {
            host_total: 64 * GIB,
            host_available: 60 * GIB,
            limit: Some(limit(256 << 20, LimitSource::Cgroup2)),
            limited_used: Some(100 << 20),
            ..MemRaw::default()
        });
        assert_eq!((m.total.raw(), m.used.raw()), (256 << 20, 100 << 20));
        assert_eq!(m.host_total.raw(), 64 * GIB);
        assert_eq!(m.source, MemorySource::Cgroup2);
    }

    #[test]
    fn test_mem_limit_at_or_above_host_is_no_limit() {
        // E-9: a 1 TiB limit on a 64 GiB host.
        let m = from_raw(MemRaw {
            host_total: 64 * GIB,
            host_available: 32 * GIB,
            limit: Some(limit(1 << 40, LimitSource::Cgroup1)),
            limited_used: Some(GIB),
            ..MemRaw::default()
        });
        assert_eq!((m.total.raw(), m.used.raw()), (64 * GIB, 32 * GIB));
        assert_eq!((m.limit, m.source), (None, MemorySource::Host));
    }

    #[test]
    fn test_mem_invariants_hold_for_inconsistent_readings() {
        // T5: available above total, usage above the limit, cache above total.
        let m = from_raw(MemRaw {
            host_total: 8 * GIB,
            host_available: 9 * GIB,
            cached: Some(20 * GIB),
            limit: Some(limit(GIB, LimitSource::JobObject)),
            limited_used: Some(2 * GIB),
            swap: Some((GIB, 2 * GIB)),
            ..MemRaw::default()
        });
        assert_eq!(m.used.saturating_add(m.free), m.total);
        assert!(m.used <= m.total && m.host_free <= m.host_total);
        assert_eq!(m.cached, Some(Bytes::new(8 * GIB)));
        assert_eq!(m.swap.used.saturating_add(m.swap.free), m.swap.total);
        assert_eq!(m.source, MemorySource::JobObject);
    }

    #[test]
    fn test_mem_lxcfs_source_kept_without_a_limit() {
        let m = from_raw(MemRaw {
            host_total: GIB,
            host_available: GIB / 2,
            source: Some(MemorySource::Lxcfs),
            ..MemRaw::default()
        });
        assert_eq!(m.source, MemorySource::Lxcfs);
    }

    #[test]
    fn test_mem_live_readings_are_consistent() {
        let m = snapshot().unwrap();
        assert!(m.total.raw() > 0 && m.used <= m.total);
        assert_eq!(m.used.saturating_add(m.free), m.total);
        assert!(m.total <= m.host_total);
        assert!(page_size().is_power_of_two() && page_size() >= 4096);
    }
}
