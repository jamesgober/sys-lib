//! This process's resource use: memory, CPU time, threads, open files,
//! limits, start time, and disk I/O, and the CPU sampler.

use core::{ops::Sub, time::Duration};
use std::time::{Instant, SystemTime};

use super::{Limit, User};
use crate::{Bytes, Error, Percent, Result};

/// This process's parent, or `None` when it has none (an orphan adopted by
/// init reports init) or the OS does not say. Windows: from a process
/// snapshot, cached after the first call.
#[must_use]
pub fn parent_id() -> Option<u32> {
    crate::sys::parent_id()
}

/// This process's resident memory (RSS): one read, no allocation.
///
/// ```
/// println!("rss {} peak {}", sys_lib::process::memory()?, sys_lib::process::peak_memory()?);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` when `/proc/self` is hidden; `Os` when the call fails.
pub fn memory() -> Result<Bytes> {
    crate::sys::process_memory()
}

/// The highest resident memory this process has reached.
///
/// # Errors
///
/// As [`memory`].
pub fn peak_memory() -> Result<Bytes> {
    crate::sys::peak_memory()
}

/// This process's memory in detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ProcessMemory {
    /// Resident in RAM.
    pub resident: Bytes,
    /// The highest resident size reached.
    pub peak_resident: Bytes,
    /// The virtual address space reserved.
    pub virtual_size: Bytes,
    /// Private (anonymous) memory; Windows: the commit charge.
    pub private: Option<Bytes>,
    /// Shared with other processes (file-backed and shared memory).
    pub shared: Option<Bytes>,
    /// Swapped out.
    pub swapped: Option<Bytes>,
}

/// This process's memory in detail. Shipped with 0.4 so that [`info`] can
/// carry it (D-044).
///
/// # Errors
///
/// As [`memory`].
pub fn memory_detail() -> Result<ProcessMemory> {
    crate::sys::memory_detail()
}

/// CPU time this process has used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CpuTime {
    /// Running user code.
    pub user: Duration,
    /// Running kernel code on this process's behalf.
    pub system: Duration,
}

impl CpuTime {
    /// User plus system time.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.user.saturating_add(self.system)
    }
}

/// The CPU time this process has used so far: one system call, no
/// allocation.
///
/// ```
/// let t = sys_lib::process::cpu_time()?;
/// println!("cpu {:?} (user {:?}, system {:?})", t.total(), t.user, t.system);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Os` when the call fails.
pub fn cpu_time() -> Result<CpuTime> {
    crate::sys::cpu_time()
}

/// This process's threads. Linux and macOS: one read, no allocation. Windows:
/// a system-wide thread snapshot, whose cost grows with the machine's thread
/// count.
///
/// # Errors
///
/// As [`memory`].
pub fn threads() -> Result<usize> {
    crate::sys::threads()
}

/// This process's open file descriptors (Unix) or handles (Windows).
///
/// ```
/// let files = sys_lib::process::limits()?.open_files;
/// println!("open files {} of {:?}", sys_lib::process::open_files()?, files.soft);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`memory`].
pub fn open_files() -> Result<usize> {
    crate::sys::open_files()
}

/// This process's resource limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Limits {
    /// Open files (`RLIMIT_NOFILE`).
    pub open_files: Limit,
    /// Processes for this user (`RLIMIT_NPROC`); Windows: the job's active
    /// process limit.
    pub processes: Limit,
    /// The address space (`RLIMIT_AS`); Windows: the job's process memory
    /// limit.
    pub address_space: Limit,
    /// Core dump size (`RLIMIT_CORE`).
    pub core_dump: Limit,
    /// The main thread's stack (`RLIMIT_STACK`).
    pub stack: Limit,
}

/// This process's resource limits. Windows has no `rlimit`: the job object's
/// limits when there is one, otherwise every value `None`.
///
/// # Errors
///
/// `Os` when a call fails.
pub fn limits() -> Result<Limits> {
    crate::sys::limits()
}

/// When this process started.
///
/// # Errors
///
/// As [`memory`].
pub fn started_at() -> Result<SystemTime> {
    crate::sys::started_at()
}

/// Everything about this process, in the fewest reads.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProcessInfo {
    /// [`super::id`].
    pub id: u32,
    /// [`parent_id`].
    pub parent_id: Option<u32>,
    /// [`memory_detail`].
    pub memory: ProcessMemory,
    /// [`cpu_time`].
    pub cpu_time: CpuTime,
    /// [`threads`], when read (Windows: only with `with_threads`).
    pub threads: Option<usize>,
    /// [`open_files`], when it could be read.
    pub open_files: Option<usize>,
    /// [`limits`].
    pub limits: Limits,
    /// [`started_at`], when it could be read.
    pub started_at: Option<SystemTime>,
    /// [`super::user`], when it could be read.
    pub user: Option<User>,
    /// [`super::is_elevated`], when it could be read.
    pub is_elevated: Option<bool>,
}

/// What [`info_with`] reads.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct InfoOptions {
    /// Count threads on Windows, which walks a system-wide snapshot (default
    /// false; Linux and macOS always count them).
    pub with_threads: bool,
}

/// Everything about this process (Windows skips the thread count).
///
/// # Errors
///
/// As [`memory`] and [`cpu_time`]; the optional readings are `None` instead
/// of failing.
pub fn info() -> Result<ProcessInfo> {
    info_with(InfoOptions::default())
}

/// Everything about this process, with `options`.
///
/// # Errors
///
/// As [`info`].
pub fn info_with(options: InfoOptions) -> Result<ProcessInfo> {
    let threads = if cfg!(windows) && !options.with_threads {
        None
    } else {
        threads().ok()
    };
    Ok(ProcessInfo {
        id: super::id(),
        parent_id: parent_id(),
        memory: memory_detail()?,
        cpu_time: cpu_time()?,
        threads,
        open_files: open_files().ok(),
        limits: limits()?,
        started_at: started_at().ok(),
        user: super::user().ok(),
        is_elevated: super::is_elevated().ok(),
    })
}

/// How much this process has read from and written to storage since it
/// started.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct ProcessIo {
    /// Bytes read from storage.
    pub read: Bytes,
    /// Bytes written to storage.
    pub written: Bytes,
    /// Read calls, when reported.
    pub read_ops: Option<u64>,
    /// Write calls, when reported.
    pub write_ops: Option<u64>,
    /// False where the OS counts all I/O, not only storage (Windows counts
    /// network and device I/O too).
    pub storage_only: bool,
}

impl Sub for ProcessIo {
    type Output = ProcessIo;

    /// The I/O between two readings, saturating at zero.
    fn sub(self, earlier: ProcessIo) -> ProcessIo {
        let ops = |a: Option<u64>, b: Option<u64>| Some(a?.saturating_sub(b?));
        ProcessIo {
            read: self.read.saturating_sub(earlier.read),
            written: self.written.saturating_sub(earlier.written),
            read_ops: ops(self.read_ops, earlier.read_ops),
            write_ops: ops(self.write_ops, earlier.write_ops),
            storage_only: self.storage_only,
        }
    }
}

/// How much this process has read from and written to storage, for a
/// database's own write amplification or a backup tool's throughput. Two
/// readings subtract; [`Rate::per`](crate::Rate::per) turns the difference
/// into a rate.
///
/// ```
/// use std::time::Duration;
/// use sys_lib::{self as sys, Rate};
///
/// let before = sys::process::io()?;
/// // ... work ...
/// let d = sys::process::io()? - before;
/// println!("wrote {} ({})", d.written, Rate::per(d.written, Duration::from_secs(1)));
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `NotFound` or `PermissionDenied` when `/proc/self/io` is hidden; `Os`.
pub fn io() -> Result<ProcessIo> {
    crate::sys::process_io()
}

/// The share of CPU this process used over an interval.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ProcessUsage {
    /// Of one core, 0.0 ..= 100.0 × logical CPUs: 250.0 is two and a half
    /// cores busy.
    pub of_one_core: f32,
    /// Of every logical CPU.
    pub of_machine: Percent,
    /// The time the values cover.
    pub interval: Duration,
}

/// Measures this process's CPU use between samples. Owned by the caller: two
/// monitors keep two samplers and never disturb each other. `sample` never
/// blocks and allocates nothing.
///
/// ```
/// use std::time::Duration;
///
/// let mut cpu = sys_lib::process::CpuSampler::new()?;
/// std::thread::sleep(Duration::from_millis(50));
/// let u = cpu.sample()?;
/// println!("this process: {:.1}% of one core, {} of the machine", u.of_one_core, u.of_machine);
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct CpuSampler {
    previous: CpuTime,
    at: Instant,
}

impl CpuSampler {
    /// Takes the first reading.
    ///
    /// # Errors
    ///
    /// As [`cpu_time`].
    pub fn new() -> Result<CpuSampler> {
        Ok(CpuSampler {
            previous: cpu_time()?,
            at: Instant::now(),
        })
    }

    /// This process's CPU use since the previous sample.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no time passed since the previous sample.
    pub fn sample(&mut self) -> Result<ProcessUsage> {
        let now = cpu_time()?;
        let at = Instant::now();
        let interval = at.duration_since(self.at);
        if interval.is_zero() {
            return Err(Error::TooSoon {
                elapsed: interval,
                minimum: Duration::from_millis(1),
            });
        }
        let used = now.total().saturating_sub(self.previous.total());
        let cpus = crate::cpu::logical() as f32;
        let of_one_core = (used.as_secs_f64() / interval.as_secs_f64() * 100.0) as f32;
        let of_one_core = of_one_core.clamp(0.0, 100.0 * cpus);
        self.previous = now;
        self.at = at;
        Ok(ProcessUsage {
            of_one_core,
            of_machine: Percent::from_value(of_one_core / cpus),
            interval,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_readings_live() {
        let rss = memory().unwrap();
        assert!(rss.raw() > 0);
        assert!(peak_memory().unwrap() >= rss || cfg!(windows));
        let d = memory_detail().unwrap();
        assert!(d.virtual_size >= d.resident || d.virtual_size.raw() == 0);
        let t = cpu_time().unwrap();
        assert!(t.total() >= t.user);
        assert!(threads().unwrap() >= 1);
        assert!(open_files().unwrap() >= 1);
        let l = limits().unwrap();
        if let (Some(s), Some(h)) = (l.open_files.soft, l.open_files.hard) {
            assert!(s <= h);
        }
        let started = started_at().unwrap();
        assert!(started <= SystemTime::now());
        let info = info().unwrap();
        assert_eq!(info.id, super::super::id());
        let io = io().unwrap();
        assert_eq!((io - io).read, Bytes::ZERO);
    }

    #[test]
    fn test_process_cpu_sampler_bounded() {
        let mut s = CpuSampler::new().unwrap();
        let mut x = 0u64;
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(30) {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let u = s.sample().unwrap();
        assert!(u.of_one_core >= 0.0 && u.of_one_core <= 100.0 * crate::cpu::logical() as f32);
        assert!(u.of_machine <= Percent::FULL);
    }

    #[test]
    fn test_process_io_subtracts_saturating() {
        let a = ProcessIo {
            read: Bytes::new(10),
            written: Bytes::new(20),
            read_ops: Some(3),
            write_ops: None,
            storage_only: true,
        };
        let b = ProcessIo {
            read: Bytes::new(15),
            written: Bytes::new(5),
            read_ops: Some(5),
            write_ops: Some(1),
            storage_only: true,
        };
        let d = b - a;
        assert_eq!(
            (d.read.raw(), d.written.raw(), d.read_ops, d.write_ops),
            (5, 0, Some(2), None)
        );
    }
}
