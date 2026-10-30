//! Host: uptime from the Mach continuous clock, the boot time, and the
//! machine's process, thread, and open-file counts.

use core::{ffi::c_int, time::Duration};
use std::{sync::OnceLock, time::SystemTime};

use super::{SendRight, ffi, kern_error};
use crate::{
    Error, Result,
    host::{OpenFiles, Processes},
};

/// The Mach timebase (never changes), or `None` when it cannot be read.
fn timebase() -> Option<ffi::MachTimebaseInfo> {
    static TIMEBASE: OnceLock<Option<ffi::MachTimebaseInfo>> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = ffi::MachTimebaseInfo::default();
        // SAFETY: `info` is a writable `mach_timebase_info`.
        let kr = unsafe { ffi::mach_timebase_info(&raw mut info) };
        (kr == ffi::KERN_SUCCESS && info.numer > 0 && info.denom > 0).then_some(info)
    })
}

/// Mach time units to nanoseconds, in 128 bits so nothing overflows.
fn mach_to_duration(ticks: u64, timebase: ffi::MachTimebaseInfo) -> Duration {
    let nanos = u128::from(ticks) * u128::from(timebase.numer) / u128::from(timebase.denom);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// `mach_continuous_time`, which counts from boot and keeps counting while the
/// machine sleeps: monotonic, unlike the wall clock. When the clock cannot be
/// read, the wall clock minus the boot time.
pub(crate) fn uptime() -> Result<Duration> {
    if let Some(tb) = timebase() {
        let up = mach_to_duration(ffi::mach_continuous_time(), tb);
        if !up.is_zero() {
            return Ok(up);
        }
    }
    SystemTime::now()
        .duration_since(boot_time()?)
        .map_err(|_| Error::Malformed {
            what: "uptime",
            source: "kern.boottime",
            line: None,
        })
}

/// `kern.boottime`, which the kernel moves with the wall clock so that it stays
/// the boot instant.
pub(crate) fn boot_time() -> Result<SystemTime> {
    const WHAT: &str = "boot time";
    const NAME: &core::ffi::CStr = c"kern.boottime";
    let tv: ffi::Timeval = super::sysctl_value(NAME).map_err(|e| e.into_error(WHAT, NAME))?;
    let malformed = || Error::Malformed {
        what: WHAT,
        source: "kern.boottime",
        line: None,
    };
    if tv.sec <= 0 || !(0..1_000_000).contains(&tv.usec) {
        return Err(malformed());
    }
    SystemTime::UNIX_EPOCH
        .checked_add(super::timeval_duration(tv))
        .ok_or_else(malformed)
}

/// The default processor set's load information: the machine's task and
/// thread counts, as `top` shows them. (`kern.num_threads` is the thread
/// limit, not the count.)
fn set_load(what: &'static str) -> Result<ffi::ProcessorSetLoadInfo> {
    let host = SendRight::host(what)?;
    let mut set: ffi::MachPort = 0;
    // SAFETY: `set` is writable; a port returned is owned by `set` below.
    let kr = unsafe { ffi::processor_set_default(host.0, &raw mut set) };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(what, "processor_set_default", kr));
    }
    let set = SendRight(set);
    let mut info = ffi::ProcessorSetLoadInfo::default();
    let mut count = ffi::PROCESSOR_SET_LOAD_INFO_COUNT;
    // SAFETY: `info` is `count` `natural_t` units of writable memory; the
    // kernel writes at most that many.
    let kr = unsafe {
        ffi::processor_set_statistics(
            set.0,
            ffi::PROCESSOR_SET_LOAD_INFO,
            (&raw mut info).cast::<c_int>(),
            &raw mut count,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(what, "processor_set_statistics", kr));
    }
    Ok(info)
}

/// The processes by `proc_listallpids` (macOS; iOS: the processor set's task
/// count), and the threads from the processor set.
pub(crate) fn processes() -> Result<Processes> {
    const WHAT: &str = "process count";
    let threads = |load: &ffi::ProcessorSetLoadInfo| u64::try_from(load.thread_count).ok();
    #[cfg(target_os = "macos")]
    let (total, threads) = (
        pid_count(WHAT)?,
        set_load(WHAT).ok().as_ref().and_then(threads),
    );
    #[cfg(not(target_os = "macos"))]
    let (total, threads) = {
        let load = set_load(WHAT)?;
        let total = u64::try_from(load.task_count).map_err(|_| Error::Malformed {
            what: WHAT,
            source: "processor_set_statistics",
            line: None,
        })?;
        (total, threads(&load))
    };
    Ok(Processes {
        total,
        threads,
        running: None,
    })
}

/// The most process ids listed; more is `TooLarge`.
#[cfg(target_os = "macos")]
const MAX_PIDS: usize = 4 << 20;

/// The number of processes. Asked with no buffer, `proc_listallpids` answers an
/// estimate with kernel slack, so the list itself is read and counted (into a
/// buffer a little larger than the estimate, read again if it fills).
#[cfg(target_os = "macos")]
fn pid_count(what: &'static str) -> Result<u64> {
    let mut pids: Vec<c_int> = Vec::new();
    let mut count = 0;
    for _ in 0..3 {
        // SAFETY: a null buffer asks for the estimate only.
        let estimate = unsafe { ffi::proc_listallpids(core::ptr::null_mut(), 0) };
        let estimate = usize::try_from(estimate)
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| super::os_error(what, "proc_listallpids", super::last_errno()))?;
        let room = estimate.saturating_add(64);
        if room > MAX_PIDS {
            return Err(Error::TooLarge {
                what,
                limit: MAX_PIDS as u64,
            });
        }
        pids.resize(room, 0);
        let bytes = c_int::try_from(room * core::mem::size_of::<c_int>()).unwrap_or(c_int::MAX);
        // SAFETY: `pids` is at least `bytes` writable bytes; the call writes at
        // most `bytes`.
        let n =
            unsafe { ffi::proc_listallpids(pids.as_mut_ptr().cast::<core::ffi::c_void>(), bytes) };
        count = usize::try_from(n)
            .map_err(|_| super::os_error(what, "proc_listallpids", super::last_errno()))?;
        if count < room {
            break;
        }
    }
    Ok(count as u64)
}

/// `kern.num_files` (open now) and `kern.maxfiles`.
pub(crate) fn host_open_files() -> Result<OpenFiles> {
    Ok(OpenFiles {
        open: super::sysctl_u64(c"kern.num_files"),
        max: super::sysctl_u64(c"kern.maxfiles"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_apple_mach_time_scaled_by_timebase() {
        let intel = ffi::MachTimebaseInfo { numer: 1, denom: 1 };
        assert_eq!(mach_to_duration(1_500, intel), Duration::from_nanos(1_500));
        // Apple silicon: 24 MHz ticks, 125/3 ns each.
        let arm = ffi::MachTimebaseInfo {
            numer: 125,
            denom: 3,
        };
        assert_eq!(mach_to_duration(24_000_000, arm), Duration::from_secs(1));
        assert_eq!(
            mach_to_duration(u64::MAX, arm),
            Duration::from_nanos(u64::MAX)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_host_apple_live_readings() {
        let up = uptime().unwrap();
        let boot = boot_time().unwrap();
        let since_boot = SystemTime::now().duration_since(boot).unwrap();
        // The two clocks agree to within a minute (the wall clock may step).
        assert!(
            up.abs_diff(since_boot) < Duration::from_secs(60),
            "{up:?} vs {since_boot:?}"
        );
        let p = processes().unwrap();
        assert!(p.total >= 2 && p.threads.is_some_and(|t| t >= p.total));
        let f = host_open_files().unwrap();
        assert!(f.open.is_some_and(|n| n > 0) && f.max.is_some_and(|n| n > 0));
    }
}
