//! Environment and reports (0.6): temperatures, graphics adapters, and the
//! settings the readiness, benchmark, and security reports read, from
//! sysctls and `getrlimit`.

use std::{ffi::CString, path::Path};

use super::{ffi, sysctl_signed, sysctl_string, sysctl_u64};
use crate::{
    Result,
    bench::BenchFacts,
    disk::Disk,
    gpu::Gpu,
    readiness::{DataDirTuning, Fact, MemLock, Tuning},
    security::{Lockdown, Posture, Tpm, TpmVersion},
    sensors::{SensorKind, Temperature, plausible},
};

/// The most CPU and thermal-zone sensors read.
const MAX_SENSORS: usize = 4096;
const MAX_ZONES: usize = 64;

/// A temperature sysctl in the kernel's `IK` format: tenths of a kelvin.
fn celsius(deci_kelvin: i64) -> Option<f32> {
    let c = deci_kelvin as f32 / 10.0 - 273.15;
    (deci_kelvin > 0 && plausible(c)).then_some(c)
}

/// One `IK` sysctl by a name built at run time; `None` when absent or not a
/// plausible reading.
fn ik(name: &str) -> Option<f32> {
    let name = CString::new(name).ok()?;
    celsius(sysctl_signed(&name).ok()?)
}

/// The core temperatures of `coretemp(4)` and `amdtemp(4)`
/// (`dev.cpu.N.temperature`, with Intel's `coretemp.tjmax` as the critical
/// point), and the ACPI thermal zones (`hw.acpi.thermal.tzN.temperature`, with
/// `_HOT` and `_CRT`). Without those drivers loaded the list is empty.
pub(crate) fn temperatures() -> Result<Vec<Temperature>> {
    let cpus = super::count(c"hw.ncpu").unwrap_or(1).min(MAX_SENSORS);
    let mut out = Vec::new();
    for n in 0..cpus {
        let Some(c) = ik(&format!("dev.cpu.{n}.temperature")) else {
            continue;
        };
        out.push(Temperature {
            label: format!("cpu{n}"),
            kind: SensorKind::Cpu,
            celsius: c,
            high: None,
            critical: ik(&format!("dev.cpu.{n}.coretemp.tjmax")),
        });
    }
    for n in 0..MAX_ZONES {
        let Some(c) = ik(&format!("hw.acpi.thermal.tz{n}.temperature")) else {
            break;
        };
        out.push(Temperature {
            label: format!("tz{n}"),
            kind: SensorKind::Other,
            celsius: c,
            high: ik(&format!("hw.acpi.thermal.tz{n}._HOT")),
            critical: ik(&format!("hw.acpi.thermal.tz{n}._CRT")),
        });
    }
    Ok(out)
}

/// The PCI device list needs `/dev/pci`, which the readings here do not open
/// yet: unknown.
pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    Err(super::unsupported("graphics adapters"))
}

/// The timecounter's name in the vocabulary the reports share (Linux's clock
/// source names, which `bench::slow_clock` knows): the TSC in either of its
/// FreeBSD forms is `tsc`, the ACPI power-management timer `acpi_pm`, the
/// 8254 `pit`; any other counter keeps its own name, lowercased.
fn clock_name(timecounter: &str) -> String {
    match timecounter {
        t if t == "TSC" || t.starts_with("TSC-") => "tsc".to_owned(),
        "HPET" => "hpet".to_owned(),
        "ACPI-fast" | "ACPI-safe" => "acpi_pm".to_owned(),
        "i8254" => "pit".to_owned(),
        "kvmclock" => "kvm-clock".to_owned(),
        "XENTIMER" => "xen".to_owned(),
        other => other.to_ascii_lowercase(),
    }
}

/// `kern.timecounter.hardware`, in the shared vocabulary.
fn clock_source() -> Option<String> {
    sysctl_string(c"kern.timecounter.hardware").map(|t| clock_name(&t))
}

/// A setting the check needs: its value, or why it could not be read.
fn fact_of(name: &'static core::ffi::CStr) -> Fact<u64> {
    match super::sysctl_int(name) {
        Ok(n) => Fact::Value(n),
        Err(e) => Fact::Unknown(e.into_error("setting", name).to_string()),
    }
}

/// The listen queue limit: `kern.ipc.soacceptqueue` (FreeBSD 10 and later),
/// else its old name `kern.ipc.somaxconn`. FreeBSD has no separate SYN
/// backlog setting (the syncache sizes itself).
fn backlog() -> Fact<(u64, Option<u64>)> {
    match super::sysctl_int(c"kern.ipc.soacceptqueue") {
        Ok(n) => Fact::Value((n, None)),
        Err(_) => match fact_of(c"kern.ipc.somaxconn") {
            Fact::Value(n) => Fact::Value((n, None)),
            Fact::Unknown(why) => Fact::Unknown(why),
            Fact::Skip => Fact::Skip,
        },
    }
}

/// `net.inet.ip.portrange.first` and `.last`, the ephemeral range; the
/// kernel accepts them in either order.
fn ports() -> Fact<(u32, u32)> {
    let read = |name| {
        super::sysctl_int(name)
            .ok()
            .and_then(|n| u32::try_from(n).ok())
    };
    match (
        read(c"net.inet.ip.portrange.first"),
        read(c"net.inet.ip.portrange.last"),
    ) {
        (Some(a), Some(b)) => Fact::Value((a.min(b), a.max(b))),
        _ => Fact::Unknown("net.inet.ip.portrange could not be read".to_owned()),
    }
}

/// `RLIMIT_MEMLOCK`'s soft value.
fn memlock() -> Fact<MemLock> {
    match super::rlimit(ffi::RLIMIT_MEMLOCK, "locked-memory limit") {
        Ok(lim) => Fact::Value(MemLock::Limit {
            soft: super::limit_value(lim.cur),
        }),
        Err(e) => Fact::Unknown(e.to_string()),
    }
}

/// The settings FreeBSD has: the open-file ceilings, the locked-memory limit,
/// the listen queue, the ephemeral ports, and the timecounter. The Linux
/// memory settings (transparent huge pages, swappiness, overcommit, map
/// count, dirty ratios, NUMA balancing) have no FreeBSD counterpart and are
/// skipped, as is the frequency governor (`powerd(8)` is a daemon, not a
/// kernel policy).
pub(crate) fn tuning() -> Tuning {
    Tuning {
        files_system: fact_of(c"kern.maxfiles"),
        files_process: fact_of(c"kern.maxfilesperproc"),
        memlock: memlock(),
        backlog: backlog(),
        ports: ports(),
        clock_source: match clock_source() {
            Some(name) => Fact::Value(name),
            None => Fact::Unknown("kern.timecounter.hardware could not be read".to_owned()),
        },
        ..Tuning::default()
    }
}

/// How the mount keeps access times: `noatime` when it is set, else every
/// access updates them (FreeBSD has no `relatime` for UFS).
fn atime_of(flags: u64) -> &'static str {
    if flags & ffi::MNT_NOATIME != 0 {
        "noatime"
    } else {
        "strictatime"
    }
}

/// The data directory's mount flags, from one `statfs`. FreeBSD has no
/// per-disk I/O scheduler setting to report.
pub(crate) fn data_dir_tuning(path: &Path, _disk: &Disk) -> DataDirTuning {
    let mut buf = [0u8; super::disk::PATH_MAX];
    let atime = match super::disk::c_path(path, &mut buf) {
        Ok(c) => match super::disk::statfs(c) {
            Ok(st) => Fact::Value(atime_of(st.flags).to_owned()),
            Err(code) => Fact::Unknown(super::os_error("mount flags", "statfs", code).to_string()),
        },
        Err(e) => Fact::Unknown(e.to_string()),
    };
    DataDirTuning {
        atime,
        scheduler: Fact::Skip,
    }
}

/// `kern.elf64.aslr.enable` (FreeBSD 13 and later; on by default for amd64
/// and arm64 from 14). Absent on older kernels: unknown.
fn aslr() -> Option<bool> {
    sysctl_u64(c"kern.elf64.aslr.enable").map(|n| n != 0)
}

/// SMT from `kern.smp.threads_per_core`, the timecounter, and ASLR. FreeBSD
/// has no frequency governor, turbo switch, throttle counter, or isolated CPU
/// list to read.
pub(crate) fn bench_facts() -> BenchFacts {
    BenchFacts {
        smt: sysctl_u64(c"kern.smp.threads_per_core")
            .filter(|&n| n > 0)
            .map(|n| n > 1),
        clock_source: clock_source(),
        aslr: aslr(),
        ..BenchFacts::default()
    }
}

/// `kern.securelevel` as a lockdown level: at 1 and above root can no longer
/// load kernel modules or write kernel memory, which is what Linux's
/// `integrity` mode prevents; reading kernel memory stays possible, so it is
/// never `Confidentiality`.
fn lockdown(securelevel: i64) -> Lockdown {
    if securelevel >= 1 {
        Lockdown::Integrity
    } else {
        Lockdown::Off
    }
}

/// Whether a newbus device exists (`dev.<driver>.<unit>`).
fn device_exists(name: &core::ffi::CStr) -> bool {
    let mut mib = [0; 8];
    super::name_to_mib(name, &mut mib).is_ok()
}

/// ASLR, the securelevel, a TPM 2.0 the `tpm(4)` drivers attached, and the
/// `ipfw` firewall when it is loaded and enabled. A machine whose firewall is
/// `pf` says nothing without `/dev/pf`, which only root opens: unknown, never
/// "off". Secure Boot, disk encryption, and the MAC framework's policies are
/// unknown.
pub(crate) fn posture() -> Posture {
    let tpm = (device_exists(c"dev.tpmcrb.0") || device_exists(c"dev.tpmtis.0")).then_some(Tpm {
        version: TpmVersion::V2_0,
    });
    let firewall = (sysctl_u64(c"net.inet.ip.fw.enable") == Some(1)).then_some(true);
    Posture {
        aslr: aslr(),
        kernel_lockdown: sysctl_signed(c"kern.securelevel").ok().map(lockdown),
        tpm,
        firewall,
        ..Posture::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn test_report_freebsd_clock_names() {
        assert_eq!(clock_name("TSC-low"), "tsc");
        assert_eq!(clock_name("TSC"), "tsc");
        assert_eq!(clock_name("HPET"), "hpet");
        assert_eq!(clock_name("ACPI-fast"), "acpi_pm");
        assert_eq!(clock_name("i8254"), "pit");
        assert_eq!(clock_name("kvmclock"), "kvm-clock");
        assert_eq!(clock_name("Hyper-V"), "hyper-v");
        assert!(crate::bench::slow_clock(&clock_name("ACPI-safe")));
        assert!(!crate::bench::slow_clock(&clock_name("TSC-low")));
    }

    #[test]
    fn test_report_freebsd_conversions() {
        assert_eq!(celsius(3231).map(|c| (c * 10.0).round()), Some(500.0));
        assert_eq!(celsius(0), None);
        assert_eq!(celsius(-1), None);
        assert_eq!(celsius(100_000), None, "not plausible");
        assert_eq!(atime_of(ffi::MNT_NOATIME | ffi::MNT_LOCAL), "noatime");
        assert_eq!(atime_of(ffi::MNT_LOCAL), "strictatime");
        assert_eq!(lockdown(-1), Lockdown::Off);
        assert_eq!(lockdown(0), Lockdown::Off);
        assert_eq!(lockdown(2), Lockdown::Integrity);
    }

    #[test]
    fn test_report_freebsd_live_settings() {
        let t = tuning();
        assert!(matches!(t.files_system, Fact::Value(n) if n > 0));
        assert!(matches!(t.files_process, Fact::Value(n) if n > 0));
        assert!(matches!(t.backlog, Fact::Value((n, None)) if n > 0));
        assert!(matches!(t.ports, Fact::Value((a, b)) if a <= b && b <= 65_535));
        assert!(matches!(t.clock_source, Fact::Value(_)));
        assert!(matches!(t.memlock, Fact::Value(_)));
        assert!(matches!(t.swappiness, Fact::Skip));
        let root = crate::disk::of(Path::new("/")).unwrap();
        let d = data_dir_tuning(Path::new("/"), &root);
        assert!(matches!(d.atime, Fact::Value(_)));
        assert!(matches!(d.scheduler, Fact::Skip));
        let b = bench_facts();
        assert!(b.clock_source.is_some());
        let p = posture();
        assert!(p.kernel_lockdown.is_some());
        assert!(temperatures().is_ok());
        assert!(matches!(gpus(), Err(Error::Unsupported { .. })));
    }
}
