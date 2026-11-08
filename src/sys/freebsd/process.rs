//! This process: `kern.proc.pid` (one `struct kinfo_proc`), `getrusage`,
//! `getrlimit`, `kern.proc.nfds`, and how it was started.

use core::ffi::c_int;
use std::time::SystemTime;

use super::{Key, SysctlError, ffi, last_errno, os_error};
use crate::{
    Bytes, Error, Result,
    process::{CpuTime, LaunchFacts, Limit, Limits, ProcessIo, ProcessMemory},
};

const MEMORY: &str = "process memory";
const KINFO: &str = "kern.proc.pid";

/// `getppid`; `None` for a process without a parent.
pub(crate) fn parent_id() -> Option<u32> {
    u32::try_from(ffi::getppid()).ok().filter(|&p| p > 0)
}

/// This process's `struct kinfo_proc`, as its bytes: read on the stack, never
/// allocated.
struct Kinfo([u8; ffi::KINFO_PROC_SIZE]);

impl Kinfo {
    /// `sysctl {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid}`; a record of another
    /// size than the layout read here is `Malformed`.
    fn own(what: &'static str) -> Result<Kinfo> {
        let mib = [
            ffi::CTL_KERN,
            ffi::KERN_PROC,
            ffi::KERN_PROC_PID,
            super::own_pid(),
        ];
        let mut buf = [0u8; ffi::KINFO_PROC_SIZE];
        let len = super::sysctl_raw(Key::Mib(&mib), Some(&mut buf))
            .map_err(|code| os_error(what, KINFO, code))?;
        Kinfo::from_bytes(&buf[..len.min(buf.len())]).ok_or(Error::Malformed {
            what,
            source: KINFO,
            line: None,
        })
    }

    /// The record, when it is whole and says it has this layout.
    fn from_bytes(data: &[u8]) -> Option<Kinfo> {
        let size = super::i32_at(data, ffi::KI_STRUCTSIZE)?;
        if usize::try_from(size).ok()? != ffi::KINFO_PROC_SIZE {
            return None;
        }
        Some(Kinfo(data.try_into().ok()?))
    }

    /// `ki_size`: the virtual size in bytes.
    fn virtual_size(&self) -> u64 {
        super::u64_at(&self.0, ffi::KI_SIZE).unwrap_or(0)
    }

    /// `ki_rssize`: the resident size, in pages.
    fn resident_pages(&self) -> u64 {
        super::i64_at(&self.0, ffi::KI_RSSIZE)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0)
    }

    /// `ki_numthreads`.
    fn threads(&self) -> Option<usize> {
        usize::try_from(super::i32_at(&self.0, ffi::KI_NUMTHREADS)?)
            .ok()
            .filter(|&n| n > 0)
    }

    /// `ki_start`: the start time as a `timeval`.
    fn start(&self) -> Option<(i64, i64)> {
        let at = ffi::KI_START;
        Some((super::i64_at(&self.0, at)?, super::i64_at(&self.0, at + 8)?))
    }
}

/// No service manager on FreeBSD marks the processes it starts (`rc(8)`
/// scripts and `daemon(8)` set nothing a child can check); the SSH session
/// and process 1 are what can be told.
pub(crate) fn launch() -> LaunchFacts {
    LaunchFacts {
        is_pid1: std::process::id() == 1,
        ssh: std::env::var_os("SSH_CONNECTION").is_some(),
        ..LaunchFacts::default()
    }
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

/// `ru_maxrss`, which FreeBSD gives in kilobytes, in bytes.
fn peak_bytes(usage: &ffi::Rusage) -> u64 {
    u64::try_from(usage.maxrss)
        .unwrap_or(0)
        .saturating_mul(1024)
}

/// Resident size: one sysctl into a stack buffer, no allocation.
pub(crate) fn process_memory() -> Result<Bytes> {
    let info = Kinfo::own(MEMORY)?;
    Ok(Bytes::new(
        info.resident_pages()
            .saturating_mul(super::page_size() as u64),
    ))
}

/// `ru_maxrss`, at least the resident size now: the kernel updates the peak
/// on statistics-clock ticks, so a process that just grew can read a peak
/// below its current size.
pub(crate) fn peak_memory() -> Result<Bytes> {
    let peak = peak_bytes(&rusage(MEMORY)?);
    Ok(Bytes::new(peak.max(process_memory()?.raw())))
}

/// Resident and virtual size from `kern.proc.pid`, the peak from
/// `getrusage`. Private, shared, and swapped are not reported per process
/// without walking the memory map.
pub(crate) fn memory_detail() -> Result<ProcessMemory> {
    let info = Kinfo::own(MEMORY)?;
    let resident = info
        .resident_pages()
        .saturating_mul(super::page_size() as u64);
    let peak = peak_bytes(&rusage(MEMORY)?);
    Ok(ProcessMemory {
        resident: Bytes::new(resident),
        // The two counters are read at different instants (T5).
        peak_resident: Bytes::new(peak.max(resident)),
        virtual_size: Bytes::new(info.virtual_size().max(resident)),
        private: None,
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

/// `ki_numthreads`: one sysctl, no allocation.
pub(crate) fn threads() -> Result<usize> {
    const WHAT: &str = "thread count";
    Kinfo::own(WHAT)?.threads().ok_or(Error::Malformed {
        what: WHAT,
        source: KINFO,
        line: None,
    })
}

/// `kern.proc.nfds` (FreeBSD 13 and later): the descriptors open in the
/// calling process, one integer. Its argument must be 0: the node answers
/// for the caller only.
pub(crate) fn open_files() -> Result<usize> {
    const WHAT: &str = "open files";
    const NAME: &core::ffi::CStr = c"kern.proc.nfds";
    let mib = [ffi::CTL_KERN, ffi::KERN_PROC, ffi::KERN_PROC_NFDS, 0];
    match super::sysctl_word(Key::Mib(&mib)) {
        Ok((buf, len)) => {
            usize::try_from(super::word_signed(buf, len)).map_err(|_| Error::Malformed {
                what: WHAT,
                source: "kern.proc.nfds",
                line: None,
            })
        }
        // An older kernel has no such node.
        Err(SysctlError::Errno(ffi::ENOENT | ffi::EINVAL)) => {
            Err(super::unsupported("open files (FreeBSD 13 or later)"))
        }
        Err(e) => Err(e.into_error(WHAT, NAME)),
    }
}

/// `getrlimit` for the five limits; `RLIM_INFINITY` is no limit.
pub(crate) fn limits() -> Result<Limits> {
    let get = |resource: c_int| -> Result<Limit> {
        let lim = super::rlimit(resource, "resource limits")?;
        Ok(Limit {
            soft: super::limit_value(lim.cur),
            hard: super::limit_value(lim.max),
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

/// A start time from seconds and microseconds since the epoch.
fn start_time(sec: i64, usec: i64) -> Option<SystemTime> {
    let sec = u64::try_from(sec).ok().filter(|&s| s > 0)?;
    let usec = u32::try_from(usec).ok().filter(|&u| u < 1_000_000)?;
    SystemTime::UNIX_EPOCH.checked_add(core::time::Duration::new(sec, usec * 1_000))
}

/// `ki_start`.
pub(crate) fn started_at() -> Result<SystemTime> {
    const WHAT: &str = "process start time";
    let info = Kinfo::own(WHAT)?;
    info.start()
        .and_then(|(sec, usec)| start_time(sec, usec))
        .ok_or(Error::Malformed {
            what: WHAT,
            source: KINFO,
            line: None,
        })
}

/// FreeBSD counts this process's block operations (`ru_inblock`,
/// `ru_oublock`), not the bytes they moved, so the bytes are unknown.
pub(crate) fn process_io() -> Result<ProcessIo> {
    Err(super::unsupported("process I/O"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(size: i32) -> Vec<u8> {
        let mut r = vec![0u8; ffi::KINFO_PROC_SIZE];
        r[..4].copy_from_slice(&size.to_ne_bytes());
        r[ffi::KI_SIZE..ffi::KI_SIZE + 8].copy_from_slice(&(64u64 << 20).to_ne_bytes());
        r[ffi::KI_RSSIZE..ffi::KI_RSSIZE + 8].copy_from_slice(&1000i64.to_ne_bytes());
        r[ffi::KI_NUMTHREADS..ffi::KI_NUMTHREADS + 4].copy_from_slice(&3i32.to_ne_bytes());
        r[ffi::KI_START..ffi::KI_START + 8].copy_from_slice(&1_700_000_000i64.to_ne_bytes());
        r[ffi::KI_START + 8..ffi::KI_START + 16].copy_from_slice(&250_000i64.to_ne_bytes());
        r
    }

    #[test]
    fn test_process_freebsd_kinfo_fields() {
        let k = Kinfo::from_bytes(&record(1088)).unwrap();
        assert_eq!(k.virtual_size(), 64 << 20);
        assert_eq!(k.resident_pages(), 1000);
        assert_eq!(k.threads(), Some(3));
        assert_eq!(k.start(), Some((1_700_000_000, 250_000)));
        // Another layout, a short record, an empty one.
        assert!(Kinfo::from_bytes(&record(1096)).is_none());
        assert!(Kinfo::from_bytes(&record(1088)[..1000]).is_none());
        assert!(Kinfo::from_bytes(&[]).is_none());
        let mut neg = record(1088);
        neg[ffi::KI_RSSIZE..ffi::KI_RSSIZE + 8].copy_from_slice(&(-5i64).to_ne_bytes());
        assert_eq!(Kinfo::from_bytes(&neg).unwrap().resident_pages(), 0);
    }

    #[test]
    fn test_process_freebsd_start_time_bounds() {
        let t = start_time(1_700_000_000, 250_000).unwrap();
        let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((d.as_secs(), d.subsec_micros()), (1_700_000_000, 250_000));
        assert_eq!(start_time(0, 0), None);
        assert_eq!(start_time(-1, 0), None);
        assert_eq!(start_time(1, 1_000_000), None);
        assert_eq!(start_time(1, -1), None);
    }

    #[test]
    fn test_process_freebsd_peak_in_kilobytes() {
        let usage = ffi::Rusage {
            maxrss: 2048,
            ..ffi::Rusage::default()
        };
        assert_eq!(peak_bytes(&usage), 2 << 20);
    }

    #[test]
    fn test_process_freebsd_live_readings() {
        let rss = process_memory().unwrap();
        assert!(rss.raw() > 0);
        let d = memory_detail().unwrap();
        assert!(d.peak_resident >= d.resident && d.virtual_size >= d.resident);
        assert!(peak_memory().unwrap() >= rss);
        assert!(parent_id().is_some());
        assert!(threads().unwrap() >= 1);
        let before = open_files().unwrap();
        let held: Vec<std::fs::File> = (0..64)
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
        let started = started_at().unwrap();
        assert!(started <= SystemTime::now());
        assert!(cpu_time().is_ok());
        assert!(matches!(process_io(), Err(Error::Unsupported { .. })));
    }
}
