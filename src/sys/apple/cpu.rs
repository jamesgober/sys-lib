//! CPU: Mach tick counters (`host_processor_info`), the load average, and the
//! frequency sysctls of Intel Macs.

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

/// The machine's counters: the sum of every processor's, in 64 bits. Not
/// `host_statistics(HOST_CPU_LOAD_INFO)`: the kernel rate-limits that call
/// for ordinary processes and answers a frequent caller with its last
/// figures, so two readings a moment apart would show no time passing.
pub(crate) fn cpu_times() -> Result<Times> {
    let (info, cpus) = load_info()?;
    Ok(split_cores(info.as_slice(), cpus, &mut [])?.0)
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
    let (info, cpus) = load_info()?;
    if cpus > out.len() {
        return Err(Error::TooLarge {
            what: "per-core CPU times",
            limit: out.len() as u64,
        });
    }
    split_cores(info.as_slice(), cpus, out)
}

/// Every processor's tick counters (`host_processor_info`), and how many
/// processors there are.
fn load_info() -> Result<(ProcessorInfo, usize)> {
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
    Ok((ProcessorInfo { addr, len }, cpus as usize))
}

/// The sum of every core's counters in the flat array (`CPU_STATE_MAX` ticks
/// per core), in 64 bits so the total does not wrap, with each core's
/// counters written to `out` as far as it reaches.
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
    let mut total = [0u64; ffi::CPU_STATE_MAX];
    let (per_core, _) = ticks.as_chunks::<{ ffi::CPU_STATE_MAX }>();
    let mut slots = out.iter_mut();
    for core in per_core.iter().take(cores) {
        let mut t = [0u64; ffi::CPU_STATE_MAX];
        for (state, &n) in core.iter().enumerate() {
            t[state] = u64::from(n.cast_unsigned());
            total[state] = total[state].saturating_add(t[state]);
        }
        if let Some(slot) = slots.next() {
            *slot = times_from(t);
        }
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
    fn test_cpu_apple_split_cores_short_buffer_still_sums_every_core() {
        // The total (`cpu_times` passes no buffer) counts every core; only
        // as many cores as fit are written.
        let ticks = [1, 2, 3, 4, 10, 20, 30, 40];
        let (total, n) = split_cores(&ticks, 2, &mut []).unwrap();
        assert_eq!(
            (n, total.user, total.nice),
            (2, Duration::from_millis(110), Duration::from_millis(440))
        );
        let mut out = [Times::default(); 1];
        let (again, _) = split_cores(&ticks, 2, &mut out).unwrap();
        assert_eq!(again, total);
        assert_eq!(out[0].user, Duration::from_millis(10));
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
