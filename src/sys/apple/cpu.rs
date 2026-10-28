//! CPU: Mach tick counters (`host_statistics`, `host_processor_info`), the load
//! average, and the frequency sysctls of Intel Macs.

use core::{ffi::c_int, time::Duration};

use super::{SendRight, ffi, kern_error};
use crate::{
    Error, Mhz, Pressure, Result,
    cpu::{CpuQuota, Frequency, Load, States, Times},
};

/// Mach CPU ticks are `CLK_TCK` (100 per second) units: 10 ms.
pub(crate) const CPU_TICK: Duration = Duration::from_millis(10);

/// Mach keeps user, system, idle, and nice only.
pub(crate) const CPU_STATES: States = States {
    iowait: false,
    steal: false,
    irq: false,
};

const WHAT: &str = "CPU times";

/// A tick count as time.
fn ticks(n: u64) -> Duration {
    Duration::from_millis(n.saturating_mul(10))
}

/// Times from the four Mach states (user, system, idle, nice), in ticks.
fn times_from(t: [u64; ffi::CPU_STATE_MAX]) -> Times {
    Times {
        user: ticks(t[ffi::CPU_STATE_USER]),
        nice: ticks(t[ffi::CPU_STATE_NICE]),
        system: ticks(t[ffi::CPU_STATE_SYSTEM]),
        idle: ticks(t[ffi::CPU_STATE_IDLE]),
        ..Times::default()
    }
}

/// The machine's counters: the kernel's sum over every processor. The sum is
/// 32-bit and wraps (every 50 days of idle time on a 10-core machine); the
/// samplers see a wrap as counters going backwards and re-base (E-20).
pub(crate) fn cpu_times() -> Result<Times> {
    let host = SendRight::host(WHAT)?;
    let mut info = ffi::CpuLoadInfo::default();
    let mut count = ffi::CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` is `count` `natural_t` units of writable memory; the
    // kernel writes at most that many and updates `count`.
    let kr = unsafe {
        ffi::host_statistics(
            host.0,
            ffi::HOST_CPU_LOAD_INFO,
            (&raw mut info).cast::<c_int>(),
            &raw mut count,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(WHAT, "host_statistics", kr));
    }
    if count < ffi::CPU_LOAD_INFO_COUNT {
        return Err(Error::Malformed {
            what: WHAT,
            source: "host_statistics",
            line: None,
        });
    }
    Ok(times_from(info.ticks.map(u64::from)))
}

/// The array `host_processor_info` maps into this task, returned with
/// `vm_deallocate` on drop.
struct ProcessorInfo {
    addr: *mut c_int,
    /// Length in `integer_t` units.
    len: u32,
}

impl ProcessorInfo {
    fn as_slice(&self) -> &[c_int] {
        if self.addr.is_null() {
            return &[];
        }
        // SAFETY: the kernel mapped `len` integers at `addr`; they stay mapped
        // until `drop`, which needs `&mut self`.
        unsafe { core::slice::from_raw_parts(self.addr, self.len as usize) }
    }
}

impl Drop for ProcessorInfo {
    fn drop(&mut self) {
        if self.addr.is_null() {
            return;
        }
        let size = (self.len as usize).saturating_mul(core::mem::size_of::<c_int>());
        // SAFETY: this value owns the mapping the kernel made: `len` integers
        // at `addr`, returned once.
        unsafe { ffi::vm_deallocate(ffi::mach_task_self_, self.addr.addr(), size) };
    }
}

pub(crate) fn cpu_times_per_core(out: &mut [Times]) -> Result<(Times, usize)> {
    let host = SendRight::host(WHAT)?;
    let mut cpus = 0u32;
    let mut addr: *mut c_int = core::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: every out-pointer is a writable local. On success the kernel
    // maps `len` integers at `addr`, owned by `info` below on every path.
    let kr = unsafe {
        ffi::host_processor_info(
            host.0,
            ffi::PROCESSOR_CPU_LOAD_INFO,
            &raw mut cpus,
            &raw mut addr,
            &raw mut len,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(WHAT, "host_processor_info", kr));
    }
    let info = ProcessorInfo { addr, len };
    split_cores(info.as_slice(), cpus as usize, out)
}

/// Each core's counters from the flat array (`CPU_STATE_MAX` ticks per core)
/// into `out`, and their sum, in 64 bits so the total does not wrap.
fn split_cores(ticks: &[c_int], cores: usize, out: &mut [Times]) -> Result<(Times, usize)> {
    if cores
        .checked_mul(ffi::CPU_STATE_MAX)
        .is_none_or(|need| need > ticks.len())
    {
        return Err(Error::Malformed {
            what: WHAT,
            source: "host_processor_info",
            line: None,
        });
    }
    if cores > out.len() {
        return Err(Error::TooLarge {
            what: "per-core CPU times",
            limit: out.len() as u64,
        });
    }
    let mut total = [0u64; ffi::CPU_STATE_MAX];
    for (core, slot) in ticks
        .chunks_exact(ffi::CPU_STATE_MAX)
        .take(cores)
        .zip(out.iter_mut())
    {
        let mut t = [0u64; ffi::CPU_STATE_MAX];
        for (state, &n) in core.iter().enumerate() {
            t[state] = u64::from(n.cast_unsigned());
            total[state] = total[state].saturating_add(t[state]);
        }
        *slot = times_from(t);
    }
    Ok((times_from(total), cores))
}

/// The processors the scheduler uses (`hw.activecpu`).
pub(crate) fn cpu_online(logical: usize) -> usize {
    super::cpu_available(logical)
}

/// macOS has no CPU quota.
pub(crate) fn cpu_quota() -> Result<Option<CpuQuota>> {
    Ok(None)
}

/// `hw.cpufrequency` and `hw.cpufrequency_max` in Hz, which Intel Macs report;
/// Apple silicon has neither sysctl, so both are `None`.
pub(crate) fn frequency() -> Frequency {
    let mhz = |name: &core::ffi::CStr| {
        super::sysctl_u64(name)
            .and_then(|hz| u32::try_from(hz.saturating_add(500_000) / 1_000_000).ok())
            .filter(|&mhz| mhz > 0)
            .map(Mhz)
    };
    Frequency {
        base: mhz(c"hw.cpufrequency"),
        max: mhz(c"hw.cpufrequency_max"),
    }
}

/// macOS does not report the current frequency.
pub(crate) fn current_frequency() -> Result<Option<Mhz>> {
    Ok(None)
}

pub(crate) fn load() -> Result<Option<Load>> {
    let mut avg = [0f64; 3];
    // SAFETY: `avg` is three writable doubles.
    let n = unsafe { ffi::getloadavg(avg.as_mut_ptr(), 3) };
    match n {
        3 => Ok(Some(Load::new(avg[0] as f32, avg[1] as f32, avg[2] as f32))),
        n if n < 0 => Err(super::os_error(
            "load average",
            "getloadavg",
            super::last_errno(),
        )),
        _ => Err(Error::Malformed {
            what: "load average",
            source: "getloadavg",
            line: None,
        }),
    }
}

/// macOS has no CPU stall measure.
pub(crate) fn cpu_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_apple_split_cores_sums_in_64_bits() {
        // Two cores: user, system, idle, nice ticks each; the second near the
        // 32-bit limit, so a 32-bit sum would wrap.
        let ticks = [10, 20, 30, 1, -2, 5, 7, 0];
        let mut out = [Times::default(); 3];
        let (total, n) = split_cores(&ticks, 2, &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out[0].user, Duration::from_millis(100));
        assert_eq!(out[0].system, Duration::from_millis(200));
        assert_eq!(out[0].idle, Duration::from_millis(300));
        assert_eq!(out[0].nice, Duration::from_millis(10));
        let big = u64::from(u32::MAX - 1);
        assert_eq!(out[1].user, Duration::from_millis(big * 10));
        assert_eq!(total.user, Duration::from_millis((big + 10) * 10));
        assert_eq!(total.idle, Duration::from_millis(370));
        assert_eq!(out[2], Times::default(), "past the cores is untouched");
    }

    #[test]
    fn test_cpu_apple_split_cores_short_buffer_too_large() {
        let ticks = [1; 8];
        let mut out = [Times::default(); 1];
        assert!(matches!(
            split_cores(&ticks, 2, &mut out),
            Err(Error::TooLarge { limit: 1, .. })
        ));
    }

    #[test]
    fn test_cpu_apple_split_cores_short_array_malformed() {
        let mut out = [Times::default(); 4];
        assert!(matches!(
            split_cores(&[1; 7], 2, &mut out),
            Err(Error::Malformed { .. })
        ));
        assert!(matches!(
            split_cores(&[], usize::MAX, &mut out),
            Err(Error::Malformed { .. })
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_cpu_apple_live_readings() {
        let t = cpu_times().unwrap();
        assert!(t.user + t.system + t.idle > Duration::ZERO);
        let mut out = vec![Times::default(); crate::cpu::logical()];
        let (total, n) = cpu_times_per_core(&mut out).unwrap();
        assert!(n >= 1 && n <= out.len());
        assert!(total.idle >= out[0].idle);
        let mut none: [Times; 0] = [];
        assert!(matches!(
            cpu_times_per_core(&mut none),
            Err(Error::TooLarge { .. })
        ));
        assert!(load().unwrap().is_some());
        let online = cpu_online(crate::cpu::logical());
        assert!(online >= 1 && online <= crate::cpu::logical());
        let f = frequency();
        if cfg!(target_arch = "aarch64") {
            assert_eq!(f, Frequency::default());
        }
    }
}
