//! CPU: counts, the job object's CPU rate, frequency, and time counters.

use std::{
    ffi::c_void,
    mem::{size_of, size_of_val},
    time::Duration,
};

use super::{Key, ffi, hundred_ns, non_negative, os_error, unsupported};
use crate::{
    Error, Mhz, Pressure, Result,
    cpu::{CpuQuota, Frequency, Load, States, Times, Vendor},
    mem::LimitSource,
    sys::CpuFacts,
};

/// The registry key of the first processor.
const CPU0: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";

/// The processors one call of the per-processor time query covers: one
/// processor group.
const GROUP: usize = 64;

/// The most processors a power-information buffer is sized for.
const MAX_CPUS: usize = 16_384;

/// The timer interrupt period: the resolution of the CPU time counters.
pub(crate) const CPU_TICK: Duration = Duration::from_micros(15_625);

/// `GetSystemTimes` gives idle, kernel, and user time only: DPC and interrupt
/// time are inside kernel time, not apart from it.
pub(crate) const CPU_STATES: States = States {
    iowait: false,
    steal: false,
    irq: false,
};

pub(crate) fn cpu() -> CpuFacts {
    let logical = usize::try_from(ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS))
        .ok()
        .filter(|&n| n > 0)
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZero::get)
        })
        .unwrap_or(1);
    let key = Key::local_machine(CPU0);
    let raw_name = key.as_ref().and_then(|k| k.string("ProcessorNameString"));
    let vendor_id = key
        .as_ref()
        .and_then(|k| k.string("VendorIdentifier"))
        .unwrap_or_default();
    let vendor = if vendor_id == "GenuineIntel" {
        Vendor::Intel
    } else if vendor_id == "AuthenticAMD" {
        Vendor::Amd
    } else if vendor_id.contains("Qualcomm")
        || raw_name
            .as_deref()
            .is_some_and(|n| n.contains("Snapdragon"))
    {
        Vendor::Qualcomm
    } else if vendor_id.starts_with("ARM") {
        Vendor::Arm
    } else {
        Vendor::Unknown
    };
    CpuFacts {
        logical,
        physical: count_relations(ffi::lpi::RELATION_PROCESSOR_CORE),
        packages: count_relations(ffi::lpi::RELATION_PROCESSOR_PACKAGE),
        raw_name,
        name: None,
        vendor,
    }
}

/// Counts the `relationship` records of `GetLogicalProcessorInformationEx`
/// (cores or packages) across every processor group.
fn count_relations(relationship: i32) -> Option<usize> {
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 asks for the size needed.
    unsafe {
        ffi::GetLogicalProcessorInformationEx(relationship, std::ptr::null_mut(), &raw mut len)
    };
    if len == 0 || len > 16 << 20 {
        return None;
    }
    // u64 elements keep the records 8-byte aligned, as the structs require.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` writable bytes.
    let ok = unsafe {
        ffi::GetLogicalProcessorInformationEx(
            relationship,
            buf.as_mut_ptr().cast::<u8>(),
            &raw mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: `buf` is `buf.len() * 8` initialised bytes; a u8 view of it is valid.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
    let count = count_records(&bytes[..(len as usize).min(bytes.len())], relationship);
    (count > 0).then_some(count)
}

/// The records of kind `relationship` in a `GetLogicalProcessorInformationEx`
/// buffer; parsing stops at the first record that does not fit.
fn count_records(bytes: &[u8], relationship: i32) -> usize {
    let mut count = 0usize;
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let field = |o: usize| {
            u32::from_ne_bytes([
                bytes[at + o],
                bytes[at + o + 1],
                bytes[at + o + 2],
                bytes[at + o + 3],
            ])
        };
        let (kind, size) = (
            field(ffi::lpi::RELATIONSHIP) as i32,
            field(ffi::lpi::SIZE) as usize,
        );
        if size == 0 || at + size > bytes.len() {
            break;
        }
        if kind == relationship {
            count += 1;
        }
        at += size;
    }
    count
}

pub(crate) fn cpu_available(logical: usize) -> usize {
    let process = ffi::GetCurrentProcess();
    let mut groups = [0u16; 64];
    let mut count = groups.len() as u16;
    // SAFETY: `groups` has `count` writable entries.
    let ok = unsafe { ffi::GetProcessGroupAffinity(process, &raw mut count, groups.as_mut_ptr()) };
    let mut cpus = if ok != 0 && count == 1 {
        let (mut mask, mut system) = (0usize, 0usize);
        // SAFETY: both masks are writable locals.
        let ok = unsafe { ffi::GetProcessAffinityMask(process, &raw mut mask, &raw mut system) };
        if ok != 0 {
            mask.count_ones() as usize
        } else {
            logical
        }
    } else if ok != 0 {
        // A process spanning several groups (Windows 11 and Server 2022 by
        // default, E-13) may use every processor of those groups.
        groups[..usize::from(count).min(groups.len())]
            .iter()
            .map(|&g| ffi::GetActiveProcessorCount(g) as usize)
            .sum()
    } else {
        logical
    };
    if let Some(rate) = job_cpu_rate() {
        let cap = (u64::from(rate) * logical as u64).div_ceil(10_000);
        cpus = cpus.min(usize::try_from(cap.max(1)).unwrap_or(usize::MAX));
    }
    cpus.clamp(1, logical.max(1))
}

/// The job object's hard CPU-rate cap: a share of every processor, in
/// hundredths of a percent. `None` outside a job, without a cap, or with a
/// weight-based (relative) rate.
fn job_cpu_rate() -> Option<u32> {
    let mut info = ffi::JobObjectCpuRateControlInformation {
        control_flags: 0,
        value: 0,
    };
    // SAFETY: a null job means the job of this process; `info` is the buffer
    // for the CPU-rate class, of the size passed.
    let ok = unsafe {
        ffi::QueryInformationJobObject(
            std::ptr::null_mut(),
            ffi::JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION,
            (&raw mut info).cast::<c_void>(),
            size_of::<ffi::JobObjectCpuRateControlInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    cap_rate(info.control_flags, info.value)
}

/// The hard cap a `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` sets, in
/// hundredths of a percent: `CpuRate` with the hard-cap flag, or `MaxRate`
/// (the high word) with the min-max flag.
fn cap_rate(flags: u32, value: u32) -> Option<u32> {
    if flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE == 0 {
        return None;
    }
    let rate = if flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP != 0 {
        value
    } else if flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE != 0 {
        value >> 16
    } else {
        return None;
    };
    (rate > 0).then_some(rate.min(10_000))
}

/// A CPU rate cap in CPUs: the rate is a share of the whole machine.
fn rate_in_cpus(rate: u32, logical: usize) -> f32 {
    (f64::from(rate) / 10_000.0 * logical as f64) as f32
}

pub(crate) fn processor_is_64bit() -> bool {
    if cfg!(target_pointer_width = "64") {
        return true;
    }
    let (mut process, mut native) = (0u16, 0u16);
    // SAFETY: both outputs are writable locals.
    let ok = unsafe {
        ffi::IsWow64Process2(ffi::GetCurrentProcess(), &raw mut process, &raw mut native)
    };
    ok != 0
        && matches!(
            native,
            ffi::IMAGE_FILE_MACHINE_AMD64 | ffi::IMAGE_FILE_MACHINE_ARM64
        )
}

pub(crate) fn cpu_online(logical: usize) -> usize {
    match ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS) {
        0 => logical,
        n => n as usize,
    }
}

pub(crate) fn cpu_quota() -> Result<Option<CpuQuota>> {
    Ok(job_cpu_rate().map(|rate| CpuQuota {
        cpus: rate_in_cpus(rate, crate::cpu::logical()),
        source: LimitSource::JobObject,
    }))
}

pub(crate) fn frequency() -> Frequency {
    let base = Key::local_machine(CPU0)
        .and_then(|k| k.dword("~MHz"))
        .filter(|&m| m > 0)
        .map(Mhz);
    let max = power_info(|cpus| cpus.iter().map(|c| c.max_mhz).max())
        .ok()
        .flatten()
        .filter(|&m| m > 0)
        .map(Mhz);
    Frequency { base, max }
}

pub(crate) fn current_frequency() -> Result<Option<Mhz>> {
    power_info(|cpus| mean_current_mhz(cpus).map(Mhz))
}

/// The mean `CurrentMhz` over the entries the OS filled (`MaxMhz` set).
fn mean_current_mhz(cpus: &[ffi::ProcessorPowerInformation]) -> Option<u32> {
    let (sum, n) = cpus
        .iter()
        .filter(|c| c.max_mhz > 0)
        .fold((0u64, 0u64), |(s, n), c| {
            (s + u64::from(c.current_mhz), n + 1)
        });
    if n == 0 {
        return None;
    }
    u32::try_from(sum / n).ok().filter(|&m| m > 0)
}

/// Calls `f` with one `PROCESSOR_POWER_INFORMATION` per processor. Up to 64
/// processors the buffer is on the stack; past that, or when the OS asks for
/// more, it is allocated (at most 3 tries).
fn power_info<T>(f: impl FnOnce(&[ffi::ProcessorPowerInformation]) -> T) -> Result<T> {
    let count =
        (ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS) as usize).clamp(1, MAX_CPUS);
    let mut stack = [ffi::ProcessorPowerInformation::default(); GROUP];
    if count <= stack.len() {
        match query_power(&mut stack) {
            Ok(()) => return Ok(f(&stack)),
            Err(status) if status != ffi::STATUS_BUFFER_TOO_SMALL => {
                return Err(power_error(status));
            }
            Err(_) => {}
        }
    }
    let mut len = count.max(GROUP * 2);
    for _ in 0..3 {
        let mut heap = vec![ffi::ProcessorPowerInformation::default(); len];
        match query_power(&mut heap) {
            Ok(()) => return Ok(f(&heap)),
            Err(status) if status != ffi::STATUS_BUFFER_TOO_SMALL => {
                return Err(power_error(status));
            }
            Err(_) => len = (len * 2).min(MAX_CPUS),
        }
    }
    Err(Error::TooLarge {
        what: "CPU frequency",
        limit: MAX_CPUS as u64,
    })
}

fn query_power(buf: &mut [ffi::ProcessorPowerInformation]) -> core::result::Result<(), i32> {
    // SAFETY: no input; `buf` is writable for the byte length passed.
    let status = unsafe {
        ffi::CallNtPowerInformation(
            ffi::PROCESSOR_INFORMATION,
            std::ptr::null(),
            0,
            buf.as_mut_ptr().cast::<c_void>(),
            u32::try_from(size_of_val(buf)).unwrap_or(u32::MAX),
        )
    };
    if status >= 0 { Ok(()) } else { Err(status) }
}

fn power_error(status: i32) -> Error {
    Error::Os {
        what: "CPU frequency",
        call: "CallNtPowerInformation",
        code: status,
    }
}

/// Windows keeps no load average; nothing is simulated.
pub(crate) fn load() -> Result<Option<Load>> {
    Ok(None)
}

/// Windows has no CPU stall measure.
pub(crate) fn cpu_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

pub(crate) fn cpu_times() -> Result<Times> {
    let (mut idle, mut kernel, mut user) = (
        ffi::FileTime::default(),
        ffi::FileTime::default(),
        ffi::FileTime::default(),
    );
    // SAFETY: the three outputs are writable locals.
    let ok = unsafe { ffi::GetSystemTimes(&raw mut idle, &raw mut kernel, &raw mut user) };
    if ok == 0 {
        return Err(os_error("CPU times", "GetSystemTimes"));
    }
    Ok(times(idle.get(), kernel.get(), user.get()))
}

/// Counters in 100 ns units as `Times`. Kernel time includes idle time, so
/// system is the difference, saturating when idle reads higher (E-26).
fn times(idle: u64, kernel: u64, user: u64) -> Times {
    Times {
        user: hundred_ns(user),
        system: hundred_ns(kernel.saturating_sub(idle)),
        idle: hundred_ns(idle),
        ..Times::default()
    }
}

/// Each processor's counters from `NtQuerySystemInformation`, into a stack
/// buffer of one processor group: no allocation. The query covers the calling
/// thread's group only, so a machine whose processors span several groups
/// (more than 64, or a NUMA layout that splits them) gets `Unsupported`; the
/// total from `GetSystemTimes` is correct either way.
pub(crate) fn cpu_times_per_core(out: &mut [Times]) -> Result<(Times, usize)> {
    let active = ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS) as usize;
    if active > GROUP {
        return Err(unsupported("per-core CPU times past 64 processors"));
    }
    let mut buf = [ffi::SystemProcessorPerformanceInformation::default(); GROUP];
    let mut returned = 0u32;
    // SAFETY: `buf` is writable for the byte length passed; `returned` is a
    // writable local.
    let status = unsafe {
        ffi::NtQuerySystemInformation(
            ffi::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
            buf.as_mut_ptr().cast::<c_void>(),
            size_of_val(&buf) as u32,
            &raw mut returned,
        )
    };
    if status < 0 {
        return Err(Error::Os {
            what: "CPU times",
            call: "NtQuerySystemInformation",
            code: status,
        });
    }
    let n =
        (returned as usize / size_of::<ffi::SystemProcessorPerformanceInformation>()).min(GROUP);
    if n == 0 {
        return Err(Error::Malformed {
            what: "CPU times",
            source: "NtQuerySystemInformation",
            line: None,
        });
    }
    if n < active {
        return Err(unsupported("per-core CPU times across processor groups"));
    }
    if n > out.len() {
        return Err(Error::TooLarge {
            what: "per-core CPU times",
            limit: out.len() as u64,
        });
    }
    let total = cpu_times()?;
    for (slot, core) in out.iter_mut().zip(&buf[..n]) {
        *slot = times(
            non_negative(core.idle_time),
            non_negative(core.kernel_time),
            non_negative(core.user_time),
        );
    }
    Ok((total, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(relationship: i32, size: u32) -> Vec<u8> {
        let mut r = vec![0u8; (size as usize).max(8)];
        r[..4].copy_from_slice(&relationship.to_ne_bytes());
        r[4..8].copy_from_slice(&size.to_ne_bytes());
        r
    }

    #[test]
    fn test_cpu_windows_records_counted_and_bounded() {
        let mut bytes = record(ffi::lpi::RELATION_PROCESSOR_PACKAGE, 80);
        bytes.extend(record(ffi::lpi::RELATION_PROCESSOR_CORE, 48));
        bytes.extend(record(ffi::lpi::RELATION_PROCESSOR_PACKAGE, 80));
        assert_eq!(
            count_records(&bytes, ffi::lpi::RELATION_PROCESSOR_PACKAGE),
            2
        );
        assert_eq!(count_records(&bytes, ffi::lpi::RELATION_PROCESSOR_CORE), 1);
        // A record claiming more bytes than remain ends the parse.
        let mut bad = record(ffi::lpi::RELATION_PROCESSOR_CORE, 48);
        bad.extend(record(ffi::lpi::RELATION_PROCESSOR_CORE, 48));
        bad.truncate(60);
        assert_eq!(count_records(&bad, ffi::lpi::RELATION_PROCESSOR_CORE), 1);
        // A zero-size record cannot loop.
        assert_eq!(
            count_records(&record(ffi::lpi::RELATION_PROCESSOR_CORE, 0), 0),
            0
        );
        assert_eq!(count_records(&[1, 2, 3], 0), 0);
    }

    #[test]
    fn test_cpu_windows_times_kernel_includes_idle_saturates() {
        // 100 ns units: idle 3 s, kernel 5 s (idle included), user 2 s.
        let t = times(30_000_000, 50_000_000, 20_000_000);
        assert_eq!(
            (t.idle, t.system, t.user),
            (
                Duration::from_secs(3),
                Duration::from_secs(2),
                Duration::from_secs(2)
            )
        );
        // E-26: idle read above kernel.
        assert_eq!(times(60, 50, 0).system, Duration::ZERO);
    }

    #[test]
    fn test_cpu_windows_rate_cap_flags_and_cpus() {
        let enable = ffi::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE;
        let hard = enable | ffi::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
        assert_eq!(cap_rate(hard, 2_500), Some(2_500));
        assert_eq!(
            cap_rate(ffi::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, 2_500),
            None
        );
        assert_eq!(
            cap_rate(enable | ffi::JOB_OBJECT_CPU_RATE_CONTROL_WEIGHT_BASED, 5),
            None
        );
        let min_max = enable | ffi::JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE;
        assert_eq!(cap_rate(min_max, (5_000 << 16) | 100), Some(5_000));
        assert_eq!(cap_rate(hard, 0), None);
        assert_eq!(cap_rate(hard, 20_000), Some(10_000));
        assert!((rate_in_cpus(2_500, 8) - 2.0).abs() < 1e-6);
        assert!((rate_in_cpus(10_000, 16) - 16.0).abs() < 1e-6);
    }

    #[test]
    fn test_cpu_windows_mean_frequency_of_filled_entries() {
        let entry = |max_mhz, current_mhz| ffi::ProcessorPowerInformation {
            max_mhz,
            current_mhz,
            ..ffi::ProcessorPowerInformation::default()
        };
        let cpus = [entry(3_000, 2_000), entry(3_000, 3_000), entry(0, 0)];
        assert_eq!(mean_current_mhz(&cpus), Some(2_500));
        assert_eq!(mean_current_mhz(&[entry(0, 0)]), None);
        assert_eq!(mean_current_mhz(&[]), None);
    }

    #[test]
    fn test_cpu_windows_live_counts_times_and_frequency() {
        let facts = cpu();
        let physical = facts.physical.unwrap();
        let packages = facts.packages.unwrap();
        assert!(physical >= 1 && physical <= facts.logical);
        assert!(packages >= 1 && packages <= physical);
        let a = cpu_available(facts.logical);
        assert!(a >= 1 && a <= facts.logical);
        assert_eq!(cpu_online(facts.logical), facts.logical);
        assert!(processor_is_64bit() || cfg!(target_pointer_width = "32"));

        let t = cpu_times().unwrap();
        assert!(t.idle > Duration::ZERO && t.user > Duration::ZERO);
        let mut out = vec![Times::default(); facts.logical];
        match cpu_times_per_core(&mut out) {
            Ok((total, n)) => {
                assert_eq!(n, facts.logical);
                assert!(total.idle >= t.idle);
                // The cores add up to about the total (read a moment apart).
                let idle: Duration = out[..n].iter().map(|c| c.idle).sum();
                assert!(idle + Duration::from_secs(5) >= t.idle);
                let mut short = vec![Times::default(); n - 1];
                assert!(matches!(
                    cpu_times_per_core(&mut short),
                    Err(Error::TooLarge { .. })
                ));
            }
            Err(Error::Unsupported { .. }) => assert!(facts.logical > GROUP),
            Err(e) => panic!("{e}"),
        }

        let f = frequency();
        if let (Some(base), Some(max)) = (f.base, f.max) {
            assert!(base.0 > 0 && max.0 > 0);
        }
        if let Some(now) = current_frequency().unwrap() {
            assert!(now.0 > 0);
        }
        assert!(cpu_quota().is_ok());
        assert!(load().unwrap().is_none() && cpu_pressure().unwrap().is_none());
    }
}
