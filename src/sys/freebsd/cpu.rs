//! CPU: the statistics-clock tick counters (`kern.cp_time`, `kern.cp_times`),
//! the load average, and the cpufreq sysctls.

use core::time::Duration;
use std::sync::OnceLock;

use super::{Key, ffi};
use crate::{
    Error, Mhz, Pressure, Result,
    cpu::{CpuQuota, Frequency, Load, States, Times},
};

/// The shortest interval a sampler can measure: one statistics-clock tick.
/// `stathz` is 127 or 128 on current kernels (about 8 ms); 10 ms covers the
/// slowest clock a kernel configures.
pub(crate) const CPU_TICK: Duration = Duration::from_millis(10);

/// The kernel counts user, nice, system, interrupt, and idle (`CPUSTATES`).
pub(crate) const CPU_STATES: States = States {
    iowait: false,
    steal: false,
    irq: true,
};

const WHAT: &str = "CPU times";
/// `CPUSTATES` (sys/resource.h): the counters per CPU, each a `long`.
const STATES: usize = 5;
const CP_USER: usize = 0;
const CP_NICE: usize = 1;
const CP_SYS: usize = 2;
const CP_INTR: usize = 3;
const CP_IDLE: usize = 4;
/// One CPU's counters, in bytes.
const PER_CPU: usize = STATES * 8;
/// The most bytes of `kern.cp_times` read: 65 536 CPUs.
const MAX_CP_TIMES: usize = PER_CPU << 16;

/// The statistics clock's rate (`kern.clockrate`), the unit of the counters.
/// Cached: it never changes.
fn stathz() -> u64 {
    static HZ: OnceLock<u64> = OnceLock::new();
    *HZ.get_or_init(|| {
        let info: Option<ffi::Clockinfo> = super::sysctl_value(Key::Name(c"kern.clockrate")).ok();
        info.and_then(|c| {
            let hz = if c.stathz > 0 { c.stathz } else { c.hz };
            u64::try_from(hz).ok()
        })
        .filter(|&hz| hz > 0)
        .unwrap_or(128)
    })
}

/// A tick count as time, at `hz` ticks a second.
fn ticks(n: u64, hz: u64) -> Duration {
    let nanos = u128::from(n) * 1_000_000_000 / u128::from(hz.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// One CPU's (or the machine's) counters from their bytes; a negative counter
/// counts as zero.
fn times_from(t: [u64; STATES], hz: u64) -> Times {
    Times {
        user: ticks(t[CP_USER], hz),
        nice: ticks(t[CP_NICE], hz),
        system: ticks(t[CP_SYS], hz),
        irq: ticks(t[CP_INTR], hz),
        idle: ticks(t[CP_IDLE], hz),
        ..Times::default()
    }
}

/// The five counters at the front of `data`.
fn counters(data: &[u8]) -> Option<[u64; STATES]> {
    let mut t = [0u64; STATES];
    for (state, slot) in t.iter_mut().enumerate() {
        let n = super::i64_at(data, state.checked_mul(8)?)?;
        *slot = u64::try_from(n).unwrap_or(0);
    }
    Some(t)
}

fn malformed(source: &'static str) -> Error {
    Error::Malformed {
        what: WHAT,
        source,
        line: None,
    }
}

/// `kern.cp_time`: the machine's counters, one call into a stack buffer.
pub(crate) fn cpu_times() -> Result<Times> {
    let mut buf = [0u8; PER_CPU];
    let len = super::sysctl_raw(Key::Name(c"kern.cp_time"), Some(&mut buf))
        .map_err(|code| super::os_error(WHAT, "kern.cp_time", code))?;
    if len != PER_CPU {
        return Err(malformed("kern.cp_time"));
    }
    let t = counters(&buf).ok_or_else(|| malformed("kern.cp_time"))?;
    Ok(times_from(t, stathz()))
}

/// `kern.cp_times`: each CPU's counters, for every CPU id up to the highest
/// (an absent CPU reads zeros).
pub(crate) fn cpu_times_per_core(out: &mut [Times]) -> Result<(Times, usize)> {
    super::with_sysctl(
        WHAT,
        "kern.cp_times",
        Key::Name(c"kern.cp_times"),
        MAX_CP_TIMES,
        |data| split_cores(data, stathz(), out),
    )
}

/// The sum of every core's counters, in 64 bits, with each core's written to
/// `out`. `TooLarge` when `out` is shorter than the number of cores.
fn split_cores(data: &[u8], hz: u64, out: &mut [Times]) -> Result<(Times, usize)> {
    if data.is_empty() || !data.len().is_multiple_of(PER_CPU) {
        return Err(malformed("kern.cp_times"));
    }
    let cores = data.len() / PER_CPU;
    if cores > out.len() {
        return Err(Error::TooLarge {
            what: "per-core CPU times",
            limit: out.len() as u64,
        });
    }
    let mut total = [0u64; STATES];
    let (per_core, _) = data.as_chunks::<PER_CPU>();
    for (core, slot) in per_core.iter().zip(out.iter_mut()) {
        let t = counters(core).ok_or_else(|| malformed("kern.cp_times"))?;
        for (sum, n) in total.iter_mut().zip(t) {
            *sum = sum.saturating_add(n);
        }
        *slot = times_from(t, hz);
    }
    Ok((times_from(total, hz), cores))
}

/// The CPUs online (`hw.ncpu`).
pub(crate) fn cpu_online(logical: usize) -> usize {
    super::count(c"hw.ncpu")
        .unwrap_or(logical)
        .min(logical.max(1))
}

/// A `racct` CPU limit needs `rctl` and a kernel built with `RACCT`; the
/// machine's own processors are the only bound read here.
pub(crate) fn cpu_quota() -> Result<Option<CpuQuota>> {
    Ok(None)
}

/// The highest level in a `dev.cpu.0.freq_levels` list (`"2400/-1 2100/-1"`,
/// MHz and milliwatts, highest first; the order is not trusted).
fn highest_level(levels: &str) -> Option<Mhz> {
    levels
        .split_ascii_whitespace()
        .take(256)
        .filter_map(|level| level.split('/').next()?.parse::<u32>().ok())
        .filter(|&mhz| mhz > 0)
        .max()
        .map(Mhz)
}

/// The base from `hw.clockrate` (the frequency measured at boot, in MHz: the
/// TSC's on x86, which runs at the nominal frequency), and the maximum from
/// the cpufreq driver's levels, when it is loaded.
pub(crate) fn frequency() -> Frequency {
    let base = super::sysctl_u64(c"hw.clockrate")
        .and_then(|mhz| u32::try_from(mhz).ok())
        .filter(|&mhz| mhz > 0)
        .map(Mhz);
    let max = super::sysctl_string(c"dev.cpu.0.freq_levels")
        .as_deref()
        .and_then(highest_level);
    Frequency { base, max }
}

/// `dev.cpu.0.freq`, which the cpufreq driver provides; `None` without it (a
/// virtual machine, or a kernel without the driver).
pub(crate) fn current_frequency() -> Result<Option<Mhz>> {
    const NAME: &core::ffi::CStr = c"dev.cpu.0.freq";
    match super::sysctl_int(NAME) {
        Ok(mhz) => Ok(u32::try_from(mhz).ok().filter(|&m| m > 0).map(Mhz)),
        Err(super::SysctlError::Errno(ffi::ENOENT)) => Ok(None),
        Err(e) => Err(e.into_error("CPU frequency", NAME)),
    }
}

/// `getloadavg`, which reads `vm.loadavg`.
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

/// FreeBSD has no CPU stall measure.
pub(crate) fn cpu_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(t: [i64; STATES]) -> Vec<u8> {
        t.iter().flat_map(|n| n.to_ne_bytes()).collect()
    }

    #[test]
    fn test_cpu_freebsd_ticks_scale_by_stathz() {
        assert_eq!(ticks(127, 127), Duration::from_secs(1));
        assert_eq!(ticks(1, 128), Duration::from_nanos(7_812_500));
        assert_eq!(ticks(u64::MAX, 1), Duration::from_nanos(u64::MAX));
        assert_eq!(ticks(5, 0), Duration::from_secs(5), "a zero rate is 1 Hz");
    }

    #[test]
    fn test_cpu_freebsd_split_cores_sums_in_64_bits() {
        let mut data = core([128, 0, 256, 64, 1280]);
        data.extend(core([i64::MAX, 1, 2, 3, -5]));
        let mut out = [Times::default(); 3];
        let (total, n) = split_cores(&data, 128, &mut out).unwrap();
        assert_eq!(n, 2);
        assert_eq!(out[0].user, Duration::from_secs(1));
        assert_eq!(out[0].system, Duration::from_secs(2));
        assert_eq!(out[0].irq, Duration::from_millis(500));
        assert_eq!(out[0].idle, Duration::from_secs(10));
        assert_eq!(out[1].idle, Duration::ZERO, "a negative counter is zero");
        assert_eq!(total.idle, Duration::from_secs(10));
        assert_eq!(total.nice, Duration::from_nanos(7_812_500));
        assert_eq!(out[2], Times::default(), "past the cores is untouched");
    }

    #[test]
    fn test_cpu_freebsd_split_cores_bounds() {
        let data = core([1, 2, 3, 4, 5]);
        let mut none: [Times; 0] = [];
        assert!(matches!(
            split_cores(&data, 128, &mut none),
            Err(Error::TooLarge { .. })
        ));
        let mut out = [Times::default(); 4];
        assert!(matches!(
            split_cores(&data[..39], 128, &mut out),
            Err(Error::Malformed { .. })
        ));
        assert!(matches!(
            split_cores(&[], 128, &mut out),
            Err(Error::Malformed { .. })
        ));
        assert_eq!(counters(&data[..32]), None);
    }

    #[test]
    fn test_cpu_freebsd_frequency_levels() {
        assert_eq!(highest_level("2400/-1 2100/-1 1800/-1"), Some(Mhz(2400)));
        assert_eq!(highest_level("800/1000 3600/35000"), Some(Mhz(3600)));
        assert_eq!(highest_level(""), None);
        assert_eq!(highest_level("x/1 0/0 -5/2"), None);
    }

    #[test]
    fn test_cpu_freebsd_live_readings() {
        let t = cpu_times().unwrap();
        assert!(t.user + t.system + t.idle > Duration::ZERO);
        let logical = crate::cpu::logical();
        let mut out = vec![Times::default(); logical.max(256)];
        let (total, n) = cpu_times_per_core(&mut out).unwrap();
        assert!(n >= 1 && n <= out.len());
        assert!(total.idle >= out[0].idle);
        let mut none: [Times; 0] = [];
        assert!(matches!(
            cpu_times_per_core(&mut none),
            Err(Error::TooLarge { .. })
        ));
        assert!(load().unwrap().is_some());
        let online = cpu_online(logical);
        assert!(online >= 1 && online <= logical);
        assert!(current_frequency().is_ok());
        assert!(stathz() > 0);
    }
}
