//! Disks and filesystems: free space, the mounted filesystems, and how hard the
//! disks are working.
//!
//! ```
//! use sys_lib as sys;
//!
//! let s = sys::disk::space(".")?;
//! println!("{} available of {} ({} used)", s.available, s.total, s.used_percent());
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::time::Duration;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use crate::{Bytes, Error, Percent, Pressure, Rate, Result};

/// The space on one filesystem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Space {
    /// The filesystem's size.
    pub total: Bytes,
    /// Every free block, the root reserve included.
    pub free: Bytes,
    /// What an unprivileged user can use (the root reserve excluded).
    pub available: Bytes,
}

impl Space {
    /// A reading with its invariants applied: `available <= free <= total`.
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))] // the platforms that sum or build these
    pub(crate) fn new(total: u64, free: u64, available: u64) -> Space {
        let free = free.min(total);
        Space {
            total: Bytes::new(total),
            free: Bytes::new(free),
            available: Bytes::new(available.min(free)),
        }
    }

    /// Space in use: `total - free`.
    #[must_use]
    pub fn used(&self) -> Bytes {
        self.total.saturating_sub(self.free)
    }

    /// [`used`](Space::used) as a share of `total`.
    #[must_use]
    pub fn used_percent(&self) -> Percent {
        self.used().percent_of(self.total)
    }
}

/// The space on the filesystem holding `path`: one system call, no
/// allocation.
///
/// This blocks if `path` is on a dead network mount: pointing it there is the
/// caller's choice.
///
/// ```
/// use sys_lib as sys;
///
/// let s = sys::disk::space(".")?;
/// if s.available < sys::Bytes::from_gib(10) { /* refuse compaction */ }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` when `path` does not exist; `PermissionDenied`; `Os` when the
/// call fails.
pub fn space(path: impl AsRef<Path>) -> Result<Space> {
    crate::sys::disk_space(path.as_ref())
}

/// One mounted filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Disk {
    /// Where it is mounted.
    pub mount_point: PathBuf,
    /// The device: `/dev/nvme0n1p2`, `C:`, `\\?\Volume{...}\`.
    pub device: String,
    /// The volume label (Windows, macOS).
    pub label: Option<String>,
    /// The filesystem type: `ext4`, `btrfs`, `apfs`, `NTFS`, `overlay`.
    pub filesystem: String,
    /// What kind of storage it is.
    pub kind: DiskKind,
    /// The space, or `None` for media not present (an empty card reader) and
    /// network mounts.
    pub space: Option<Space>,
    /// Mounted read-only.
    pub read_only: bool,
    /// On removable media.
    pub removable: bool,
}

/// What kind of storage a [`Disk`] is on. Telling solid-state from rotating
/// disks arrives in 0.5 (DSK-04); until then a fixed local disk is `Unknown`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum DiskKind {
    /// Solid-state.
    Ssd,
    /// Rotating.
    Hdd,
    /// Removable media (USB sticks, card readers).
    Removable,
    /// A network filesystem.
    Network,
    /// Optical media.
    Optical,
    /// Memory-backed (`tmpfs`, a RAM disk).
    Ram,
    /// A virtual disk or filesystem (an overlay root in a container).
    Virtual,
    /// Not known.
    Unknown,
}

/// Which filesystems [`list_with`] includes.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ListOptions {
    /// Include network filesystems (default false: `statvfs` on a dead NFS
    /// mount can hang).
    pub include_network: bool,
    /// Include pseudo filesystems: `proc`, `sysfs`, `cgroup`, `tmpfs`,
    /// `devtmpfs`, `squashfs`, ... (default false).
    pub include_pseudo: bool,
    /// Include bind mounts and further mounts of one filesystem (default
    /// false: each filesystem once, by device number).
    pub include_duplicates: bool,
    /// The most mounts read; more is `TooLarge` (default 4 096).
    pub max_mounts: usize,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions {
            include_network: false,
            include_pseudo: false,
            include_duplicates: false,
            max_mounts: 4_096,
        }
    }
}

/// The local, real filesystems, sorted by mount point, each once.
///
/// Never blocks on a dead network mount (network filesystems are left out),
/// and on Windows never pops up "insert a disk" for an empty drive (threat
/// T8). Inside a container it shows the container's mounts.
///
/// ```
/// for d in sys_lib::disk::list()? {
///     println!("{:<20} {:<8} {:?} {:?}", d.mount_point.display(), d.filesystem, d.kind, d.space.map(|s| s.available));
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `TooLarge` with more than 4 096 mounts; `NotFound` when the mount table is
/// hidden.
pub fn list() -> Result<Vec<Disk>> {
    list_with(ListOptions::default())
}

/// The filesystems `options` selects, sorted by mount point.
///
/// ```
/// use sys_lib::disk::{self, ListOptions};
///
/// let mut options = ListOptions::default();
/// options.include_network = true;      // accept the risk of a hang on a dead mount
/// let all = disk::list_with(options)?;
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`list`], with `options.max_mounts` as the budget.
pub fn list_with(options: ListOptions) -> Result<Vec<Disk>> {
    let mut disks = crate::sys::disks(&options)?;
    disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    Ok(disks)
}

/// The interval [`io`] measures over: 1 s.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(1);

const MIN_WINDOW: Duration = Duration::from_millis(100);
const MAX_WINDOW: Duration = Duration::from_secs(60);

/// How hard the disks worked over an interval.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct DiskIo {
    /// Read speed.
    pub read: Rate,
    /// Write speed.
    pub write: Rate,
    /// Read operations per second.
    pub reads_per_sec: f64,
    /// Write operations per second.
    pub writes_per_sec: f64,
    /// The share of the interval with I/O in flight, when reported.
    pub busy: Option<Percent>,
    /// The time per completed operation, when reported.
    pub average_latency: Option<Duration>,
    /// The time the values cover.
    pub interval: Duration,
}

/// Cumulative I/O counters since boot.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct IoTotals {
    /// Bytes read.
    pub read: Bytes,
    /// Bytes written.
    pub written: Bytes,
    /// Read operations completed.
    pub reads: u64,
    /// Write operations completed.
    pub writes: u64,
    /// Time with I/O in flight, when reported.
    pub io_time: Option<Duration>,
}

/// One disk's rates.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DeviceIo {
    /// The disk: `nvme0n1`, `disk0`, `PhysicalDrive0`.
    pub device: String,
    /// Its rates.
    pub io: DiskIo,
}

/// One disk's cumulative counters.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IoCounters {
    /// The disk: `nvme0n1`, `disk0`, `PhysicalDrive0`.
    pub device: String,
    /// Its counters.
    pub totals: IoTotals,
}

/// Cumulative counters as a platform reports them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct DiskRaw {
    pub(crate) read: u64,
    pub(crate) written: u64,
    pub(crate) reads: u64,
    pub(crate) writes: u64,
    /// Time with I/O in flight.
    pub(crate) busy: Option<Duration>,
    /// Time spent on completed operations (read plus write).
    pub(crate) op_time: Option<Duration>,
    /// How many disks the counters cover (1 for one disk), so a sum's busy
    /// time becomes the mean share across disks.
    pub(crate) disks: u32,
}

impl DiskRaw {
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))] // the platforms that sum or build these
    pub(crate) fn add(&mut self, other: &DiskRaw) {
        self.read = self.read.saturating_add(other.read);
        self.written = self.written.saturating_add(other.written);
        self.reads = self.reads.saturating_add(other.reads);
        self.writes = self.writes.saturating_add(other.writes);
        self.busy = add_opt(self.busy, other.busy);
        self.op_time = add_opt(self.op_time, other.op_time);
        self.disks = self.disks.saturating_add(other.disks);
    }

    fn totals(&self) -> IoTotals {
        IoTotals {
            read: Bytes::new(self.read),
            written: Bytes::new(self.written),
            reads: self.reads,
            writes: self.writes,
            io_time: self.busy,
        }
    }

    /// The rates between `earlier` and `self`. A counter that went backwards
    /// (a disk removed and re-attached) counts as no activity: the caller
    /// re-bases.
    fn rates_since(&self, earlier: &DiskRaw, interval: Duration) -> DiskIo {
        let d = |new: u64, old: u64| new.saturating_sub(old);
        let secs = interval.as_secs_f64();
        let ops = d(self.reads, earlier.reads).saturating_add(d(self.writes, earlier.writes));
        let per_sec = |n: u64| if secs > 0.0 { n as f64 / secs } else { 0.0 };
        DiskIo {
            read: Rate::from_delta(d(self.read, earlier.read), interval),
            write: Rate::from_delta(d(self.written, earlier.written), interval),
            reads_per_sec: per_sec(d(self.reads, earlier.reads)),
            writes_per_sec: per_sec(d(self.writes, earlier.writes)),
            // Several disks: the mean share of the interval each was busy.
            busy: match (self.busy, earlier.busy) {
                (Some(n), Some(o)) if !interval.is_zero() => Some(Percent::from_ratio_u128(
                    n.checked_sub(o).unwrap_or_default().as_nanos(),
                    interval.as_nanos() * u128::from(self.disks.max(1)),
                )),
                _ => None,
            },
            average_latency: match (self.op_time, earlier.op_time) {
                (Some(n), Some(o)) if ops > 0 => {
                    let spent = n.checked_sub(o).unwrap_or_default();
                    Some(spent / u32::try_from(ops).unwrap_or(u32::MAX))
                }
                _ => None,
            },
            interval,
        }
    }
}

#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn add_opt(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.saturating_add(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Disk activity over the next second, every physical disk together.
/// **Blocks for [`DEFAULT_WINDOW`]**; a monitor keeps an [`IoSampler`].
///
/// Only physical disks are counted (partitions, device-mapper and RAID
/// members, loop, RAM, and zram devices are left out), so nothing is counted
/// twice. Inside a container the disks are the host's; this process's own I/O
/// is [`process::io`](crate::process::io).
///
/// ```
/// let io = sys_lib::disk::io()?;            // blocks 1 s
/// println!("read {} write {} ({:.0} IOPS, busy {:?})", io.read, io.write, io.reads_per_sec + io.writes_per_sec, io.busy);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`IoSampler::sample`].
pub fn io() -> Result<DiskIo> {
    io_over(DEFAULT_WINDOW)
}

/// Disk activity over `window`, clamped to 100 ms ..= 60 s. **Blocks for the
/// window.**
///
/// # Errors
///
/// As [`IoSampler::sample`].
pub fn io_over(window: Duration) -> Result<DiskIo> {
    let mut sampler = IoSampler::totals_only()?;
    std::thread::sleep(window.clamp(MIN_WINDOW, MAX_WINDOW));
    sampler.sample()
}

/// Bytes and operations since boot, every physical disk together: one read,
/// no waiting, no allocation.
///
/// # Errors
///
/// `NotFound` when the counters are hidden; `Malformed`; `Unsupported` where
/// the OS gives no disk counters to an unprivileged process.
pub fn io_totals() -> Result<IoTotals> {
    crate::sys::disk_io_totals().map(|r| r.totals())
}

/// The cumulative counters of each physical disk (Tier 2).
///
/// # Errors
///
/// As [`io_totals`].
pub fn io_counters() -> Result<Vec<IoCounters>> {
    Ok(crate::sys::disk_io_counters()?
        .into_iter()
        .map(|(device, raw)| IoCounters {
            device,
            totals: raw.totals(),
        })
        .collect())
}

/// How much time tasks were stalled on I/O ([`Pressure`]). Linux: the cgroup's
/// `io.pressure`, else `/proc/pressure/io`. macOS and Windows: `None`.
///
/// ```
/// use sys_lib::{self as sys, PressureLevel};
///
/// if let Some(p) = sys::disk::pressure()? {
///     if p.level >= PressureLevel::Warning { /* postpone compaction */ }
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Malformed` when the kernel's file does not parse.
pub fn pressure() -> Result<Option<Pressure>> {
    crate::sys::disk_pressure()
}

/// Measures disk activity between samples. Owned by the caller. `sample`
/// never blocks and allocates nothing.
///
/// ```no_run
/// use std::time::Duration;
///
/// let mut disks = sys_lib::disk::IoSampler::new()?;
/// std::thread::sleep(Duration::from_secs(5));
/// let io = disks.sample()?;
/// println!("read {} write {}", io.read, io.write);
/// for d in disks.sample_per_disk()? {
///     println!("  {}: busy {:?}", d.device, d.io.busy);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct IoSampler {
    total: DiskRaw,
    at: Instant,
    devices: Vec<(String, DiskRaw)>,
    devices_at: Instant,
}

impl IoSampler {
    /// Takes the first reading, of the total and of each disk.
    ///
    /// # Errors
    ///
    /// As [`io_totals`].
    pub fn new() -> Result<IoSampler> {
        let now = Instant::now();
        Ok(IoSampler {
            total: crate::sys::disk_io_totals()?,
            at: now,
            devices: crate::sys::disk_io_counters()?,
            devices_at: now,
        })
    }

    fn totals_only() -> Result<IoSampler> {
        let now = Instant::now();
        Ok(IoSampler {
            total: crate::sys::disk_io_totals()?,
            at: now,
            devices: Vec::new(),
            devices_at: now,
        })
    }

    /// Activity since the previous sample, every physical disk together. A
    /// counter that went backwards re-bases the sampler and reads as no
    /// activity for that interval.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample(&mut self) -> Result<DiskIo> {
        let now = crate::sys::disk_io_totals()?;
        let at = Instant::now();
        let interval = at.duration_since(self.at);
        if interval.is_zero() {
            return Err(too_soon(interval));
        }
        let io = now.rates_since(&self.total, interval);
        self.total = now;
        self.at = at;
        Ok(io)
    }

    /// Activity since the previous per-disk sample, for each disk (allocates
    /// the `Vec`). A disk seen for the first time reads as no activity.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample_per_disk(&mut self) -> Result<Vec<DeviceIo>> {
        let now = crate::sys::disk_io_counters()?;
        let at = Instant::now();
        let interval = at.duration_since(self.devices_at);
        if interval.is_zero() {
            return Err(too_soon(interval));
        }
        let out = now
            .iter()
            .map(|(device, raw)| {
                let earlier = self
                    .devices
                    .iter()
                    .find(|(d, _)| d == device)
                    .map_or(*raw, |(_, r)| *r);
                DeviceIo {
                    device: device.clone(),
                    io: raw.rates_since(&earlier, interval),
                }
            })
            .collect();
        self.devices = now;
        self.devices_at = at;
        Ok(out)
    }
}

fn too_soon(elapsed: Duration) -> Error {
    Error::TooSoon {
        elapsed,
        minimum: Duration::from_millis(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_busy_across_two_disks_is_the_mean() {
        let one = |busy_ms: u64| DiskRaw {
            busy: Some(Duration::from_millis(busy_ms)),
            disks: 1,
            ..DiskRaw::default()
        };
        let (mut before, mut after) = (one(0), one(600));
        before.add(&one(0));
        after.add(&one(600));
        let io = after.rates_since(&before, Duration::from_secs(1));
        assert_eq!(io.busy.unwrap().to_string(), "60.0%");
    }

    #[test]
    fn test_disk_space_invariants() {
        let s = Space::new(100, 150, 200);
        assert_eq!(
            (s.total.raw(), s.free.raw(), s.available.raw()),
            (100, 100, 100)
        );
        let s = Space::new(1000, 400, 300);
        assert_eq!(s.used().raw(), 600);
        assert_eq!(s.used_percent().to_string(), "60.0%");
        assert_eq!(Space::new(0, 0, 0).used_percent(), Percent::ZERO);
    }

    #[test]
    fn test_disk_rates_and_backwards_counters() {
        let a = DiskRaw {
            read: 1 << 20,
            written: 0,
            reads: 10,
            writes: 0,
            busy: Some(Duration::from_millis(100)),
            op_time: Some(Duration::from_millis(20)),
            disks: 1,
        };
        let b = DiskRaw {
            read: 3 << 20,
            written: 1 << 20,
            reads: 30,
            writes: 10,
            busy: Some(Duration::from_millis(600)),
            op_time: Some(Duration::from_millis(80)),
            disks: 1,
        };
        let io = b.rates_since(&a, Duration::from_secs(1));
        assert_eq!(io.read.to_string(), "2 MiB/s");
        assert_eq!(io.write.to_string(), "1 MiB/s");
        assert!((io.reads_per_sec - 20.0).abs() < 1e-9 && (io.writes_per_sec - 10.0).abs() < 1e-9);
        assert_eq!(io.busy.unwrap().to_string(), "50.0%");
        assert_eq!(io.average_latency, Some(Duration::from_millis(2)));
        // A disk re-attached: every counter went backwards. No negative rates.
        let back = a.rates_since(&b, Duration::from_secs(1));
        assert_eq!((back.read, back.write), (Rate::ZERO, Rate::ZERO));
        assert_eq!(back.busy.unwrap(), Percent::ZERO);
        assert_eq!(back.average_latency, None);
    }

    #[test]
    fn test_disk_live_space_and_list() {
        let s = space(".").unwrap();
        assert!(s.total.raw() > 0 && s.available <= s.free && s.free <= s.total);
        let disks = list().unwrap();
        assert!(
            disks
                .windows(2)
                .all(|w| w[0].mount_point <= w[1].mount_point)
        );
        for d in &disks {
            assert!(!d.filesystem.is_empty(), "{d:?}");
        }
        assert!(matches!(space("/definitely/not/here/sys-lib"), Err(e) if e.is_unavailable()));
    }
}
