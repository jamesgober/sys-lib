//! This process: Mach `task_info`, `getrusage`, `getrlimit`, and (macOS only)
//! libproc for threads, descriptors, the start time, and disk I/O.

use core::ffi::c_int;
#[cfg(target_os = "macos")]
use core::ffi::c_void;
use std::time::SystemTime;

use super::{ffi, kern_error, last_errno, os_error};
#[cfg(target_os = "macos")]
use crate::Error;
use crate::{
    Bytes, Result,
    process::{CpuTime, Limit, Limits, ProcessIo, ProcessMemory},
};

const MEMORY: &str = "process memory";

/// `getppid`; `None` for a process without a parent (pid 0 answers 0).
pub(crate) fn parent_id() -> Option<u32> {
    u32::try_from(ffi::getppid()).ok().filter(|&p| p > 0)
}

/// `task_info(MACH_TASK_BASIC_INFO)` for this task.
fn task_basic_info() -> Result<ffi::MachTaskBasicInfo> {
    let mut info = ffi::MachTaskBasicInfo::default();
    let mut count = ffi::MACH_TASK_BASIC_INFO_COUNT;
    // SAFETY: `info` is `count` `natural_t` units of writable memory; the
    // kernel writes at most that many. `mach_task_self_` needs no release.
    let kr = unsafe {
        ffi::task_info(
            ffi::mach_task_self_,
            ffi::MACH_TASK_BASIC_INFO,
            (&raw mut info).cast::<c_int>(),
            &raw mut count,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(MEMORY, "task_info", kr));
    }
    Ok(info)
}

/// `getrusage(RUSAGE_SELF)`.
fn rusage(what: &'static str) -> Result<ffi::Rusage> {
    let mut usage = ffi::Rusage::default();
    // SAFETY: `usage` is a writable `rusage`.
    let rc = unsafe { ffi::getrusage(ffi::RUSAGE_SELF, &raw mut usage) };
    if rc != 0 {
        return Err(os_error(what, "getrusage", last_errno()));
    }
    Ok(usage)
}

/// Resident size: one Mach call, no allocation.
pub(crate) fn process_memory() -> Result<Bytes> {
    Ok(Bytes::new(task_basic_info()?.resident_size))
}

/// `ru_maxrss`, which Apple platforms give in bytes.
pub(crate) fn peak_memory() -> Result<Bytes> {
    let usage = rusage(MEMORY)?;
    Ok(Bytes::new(u64::try_from(usage.maxrss).unwrap_or(0)))
}

/// Resident, peak, and virtual size from Mach and `getrusage`; private is the
/// physical footprint (what Activity Monitor shows) on macOS. Shared and
/// swapped are not reported per process.
pub(crate) fn memory_detail() -> Result<ProcessMemory> {
    let info = task_basic_info()?;
    let resident = info.resident_size;
    let peak = u64::try_from(rusage(MEMORY)?.maxrss).unwrap_or(0);
    #[cfg(target_os = "macos")]
    let private = rusage_v2(MEMORY).ok().map(|r| Bytes::new(r.phys_footprint));
    #[cfg(not(target_os = "macos"))]
    let private = None;
    Ok(ProcessMemory {
        resident: Bytes::new(resident),
        // The two counters are read at different instants (T5).
        peak_resident: Bytes::new(peak.max(resident)),
        virtual_size: Bytes::new(info.virtual_size),
        private,
        shared: None,
        swapped: None,
    })
}

pub(crate) fn cpu_time() -> Result<CpuTime> {
    let usage = rusage("process CPU time")?;
    Ok(CpuTime {
        user: super::timeval_duration(usage.utime),
        system: super::timeval_duration(usage.stime),
    })
}

#[cfg(target_os = "macos")]
// SAFETY: `proc_taskinfo` is integers only.
unsafe impl super::Plain for ffi::ProcTaskinfo {}
#[cfg(target_os = "macos")]
// SAFETY: `proc_bsdinfo` is integers and character arrays only.
unsafe impl super::Plain for ffi::ProcBsdinfo {}

/// `proc_pidinfo` for a fixed-size flavour of this process; `T` is the
/// flavour's struct.
#[cfg(target_os = "macos")]
fn pid_info<T: super::Plain>(what: &'static str, flavor: c_int) -> Result<T> {
    let mut info = T::default();
    let size = c_int::try_from(core::mem::size_of::<T>()).unwrap_or(c_int::MAX);
    // SAFETY: `info` is `size` writable bytes; the kernel writes at most
    // `size`, and `Plain` makes any bytes it writes a valid `T`.
    let n = unsafe {
        ffi::proc_pidinfo(
            super::own_pid(),
            flavor,
            0,
            (&raw mut info).cast::<c_void>(),
            size,
        )
    };
    if n <= 0 {
        return Err(os_error(what, "proc_pidinfo", last_errno()));
    }
    if n != size {
        return Err(Error::Malformed {
            what,
            source: "proc_pidinfo",
            line: None,
        });
    }
    Ok(info)
}

/// `proc_pid_rusage(RUSAGE_INFO_V2)` for this process.
#[cfg(target_os = "macos")]
fn rusage_v2(what: &'static str) -> Result<ffi::RusageInfoV2> {
    let mut info = ffi::RusageInfoV2::default();
    // SAFETY: `info` is a writable `rusage_info_v2`, the size of this flavour;
    // the call's `rusage_info_t *` parameter takes the struct's address.
    let rc = unsafe {
        ffi::proc_pid_rusage(
            super::own_pid(),
            ffi::RUSAGE_INFO_V2,
            (&raw mut info).cast::<*mut c_void>(),
        )
    };
    if rc != 0 {
        return Err(os_error(what, "proc_pid_rusage", last_errno()));
    }
    Ok(info)
}

/// `pti_threadnum`: one call, no allocation.
#[cfg(target_os = "macos")]
pub(crate) fn threads() -> Result<usize> {
    const WHAT: &str = "thread count";
    let info: ffi::ProcTaskinfo = pid_info(WHAT, ffi::PROC_PIDTASKINFO)?;
    usize::try_from(info.threadnum).map_err(|_| Error::Malformed {
        what: WHAT,
        source: "proc_pidinfo",
        line: None,
    })
}

/// libproc is not public on iOS.
#[cfg(not(target_os = "macos"))]
pub(crate) fn threads() -> Result<usize> {
    Err(super::unsupported("thread count"))
}

/// Descriptors in a 4 KiB stack buffer cover 512; more go to the heap.
#[cfg(target_os = "macos")]
const FD_STACK: usize = 512;
/// The most descriptors listed; more is `TooLarge`.
#[cfg(target_os = "macos")]
const FD_MAX: usize = 1 << 20;

/// The open descriptors from `proc_pidinfo(PROC_PIDLISTFDS)`.
///
/// Asked with no buffer, the kernel answers the descriptor table's size plus
/// slack, not the number open, so the list itself is read and its length
/// counted: into a stack buffer for up to 512 descriptors (no allocation),
/// else a heap buffer bounded at a million.
#[cfg(target_os = "macos")]
pub(crate) fn open_files() -> Result<usize> {
    const WHAT: &str = "open files";
    const ENTRY: usize = core::mem::size_of::<ffi::ProcFdinfo>();
    let pid = super::own_pid();
    // `proc_fdinfo` is two 32-bit integers: one `u64` per entry holds it.
    let mut stack = [0u64; FD_STACK];
    let mut heap: Vec<u64> = Vec::new();
    let mut count = 0;
    for _ in 0..3 {
        // SAFETY: a null buffer asks for the size the list may need.
        let need =
            unsafe { ffi::proc_pidinfo(pid, ffi::PROC_PIDLISTFDS, 0, core::ptr::null_mut(), 0) };
        let need = usize::try_from(need)
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| os_error(WHAT, "proc_pidinfo", last_errno()))?;
        let entries = need.div_ceil(ENTRY);
        let buf: &mut [u64] = if entries <= FD_STACK {
            &mut stack
        } else {
            let room = entries.saturating_add(64);
            if room > FD_MAX {
                return Err(Error::TooLarge {
                    what: WHAT,
                    limit: FD_MAX as u64,
                });
            }
            heap.resize(room, 0);
            &mut heap
        };
        let bytes = c_int::try_from(core::mem::size_of_val(buf)).unwrap_or(c_int::MAX);
        // SAFETY: `buf` is at least `bytes` writable bytes; the kernel writes
        // at most `bytes`.
        let got = unsafe {
            ffi::proc_pidinfo(
                pid,
                ffi::PROC_PIDLISTFDS,
                0,
                buf.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        let got = usize::try_from(got).map_err(|_| os_error(WHAT, "proc_pidinfo", last_errno()))?;
        count = got / ENTRY;
        // A full buffer may have cut the list short: ask again.
        if got.saturating_add(ENTRY) <= core::mem::size_of_val(buf) {
            break;
        }
    }
    Ok(count)
}

/// libproc is not public on iOS.
#[cfg(not(target_os = "macos"))]
pub(crate) fn open_files() -> Result<usize> {
    Err(super::unsupported("open files"))
}

/// `getrlimit` for the five limits; `RLIM_INFINITY` is no limit.
pub(crate) fn limits() -> Result<Limits> {
    let get = |resource: c_int| -> Result<Limit> {
        let lim = super::rlimit(resource, "resource limits")?;
        let value = |v: u64| (v < ffi::RLIM_INFINITY).then_some(v);
        Ok(Limit {
            soft: value(lim.cur),
            hard: value(lim.max),
        })
    };
    Ok(Limits {
        open_files: get(ffi::RLIMIT_NOFILE)?,
        processes: get(ffi::RLIMIT_NPROC)?,
        address_space: get(ffi::RLIMIT_AS)?,
        core_dump: get(ffi::RLIMIT_CORE)?,
        stack: get(ffi::RLIMIT_STACK)?,
    })
}

/// `pbi_start_tvsec` and `pbi_start_tvusec`.
#[cfg(target_os = "macos")]
pub(crate) fn started_at() -> Result<SystemTime> {
    const WHAT: &str = "process start time";
    let info: ffi::ProcBsdinfo = pid_info(WHAT, ffi::PROC_PIDTBSDINFO)?;
    start_time(info.start_tvsec, info.start_tvusec).ok_or(Error::Malformed {
        what: WHAT,
        source: "proc_pidinfo",
        line: None,
    })
}

/// libproc is not public on iOS.
#[cfg(not(target_os = "macos"))]
pub(crate) fn started_at() -> Result<SystemTime> {
    Err(super::unsupported("process start time"))
}

/// A start time from seconds and microseconds since the epoch.
#[cfg(any(target_os = "macos", test))]
fn start_time(sec: u64, usec: u64) -> Option<SystemTime> {
    if sec == 0 || usec >= 1_000_000 {
        return None;
    }
    let usec = u32::try_from(usec).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(core::time::Duration::new(sec, usec * 1_000))
}

/// Storage bytes read and written, as the kernel counts them for this process.
#[cfg(target_os = "macos")]
pub(crate) fn process_io() -> Result<ProcessIo> {
    let info = rusage_v2("process I/O")?;
    Ok(ProcessIo {
        read: Bytes::new(info.diskio_bytesread),
        written: Bytes::new(info.diskio_byteswritten),
        read_ops: None,
        write_ops: None,
        storage_only: true,
    })
}

/// libproc is not public on iOS.
#[cfg(not(target_os = "macos"))]
pub(crate) fn process_io() -> Result<ProcessIo> {
    Err(super::unsupported("process I/O"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_apple_start_time_bounds() {
        let t = start_time(1_700_000_000, 250_000).unwrap();
        let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((d.as_secs(), d.subsec_micros()), (1_700_000_000, 250_000));
        assert_eq!(start_time(0, 0), None);
        assert_eq!(start_time(1, 1_000_000), None);
        assert_eq!(start_time(u64::MAX, 0), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_process_apple_live_readings() {
        let rss = process_memory().unwrap();
        assert!(rss.raw() > 0);
        assert!(peak_memory().unwrap() >= rss);
        let d = memory_detail().unwrap();
        assert!(d.peak_resident >= d.resident && d.virtual_size >= d.resident);
        assert!(d.private.is_some_and(|p| p.raw() > 0));
        assert!(parent_id().is_some());
        assert!(threads().unwrap() >= 1);
        let before = open_files().unwrap();
        let held: Vec<std::fs::File> = (0..600)
            .filter_map(|_| std::fs::File::open("/dev/null").ok())
            .collect();
        let after = open_files().unwrap();
        // Other tests may close a few descriptors meanwhile.
        assert!(after + 16 >= before + held.len(), "{before} -> {after}");
        drop(held);
        let l = limits().unwrap();
        if let (Some(s), Some(h)) = (l.open_files.soft, l.open_files.hard) {
            assert!(s <= h);
        }
        assert!(started_at().unwrap() <= SystemTime::now());
        let io = process_io().unwrap();
        assert!(io.storage_only && io.read_ops.is_none());
    }
}
