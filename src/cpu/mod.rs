//! The processor: architecture and bitness, name and vendor, the counts
//! (cores, hardware threads, sockets, and what this process may use), the
//! frequency, usage, load, pressure, and instruction-set features.
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("{} ({:?}, {})", sys::cpu::name(), sys::cpu::vendor(), sys::cpu::arch().name());
//! println!("{} cores, {} threads, {} usable here", sys::cpu::physical(), sys::cpu::logical(), sys::cpu::available());
//! // Size a worker pool by what the container allows, not by the host.
//! let workers = sys::cpu::available();
//! # let _ = workers;
//! ```

use core::fmt;
use std::sync::OnceLock;

use crate::{Mhz, Pressure, Result, mem::LimitSource, os::keep, parse::cpu_name};

mod features;
mod usage;

pub use features::{Feature, Features, X86Level, features, has, x86_level};
pub(crate) use usage::States;
pub use usage::{DEFAULT_WINDOW, Sampler, Times, Usage, free, times, usage, usage_over, used};

/// A processor architecture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Arch {
    /// x86-64.
    X86_64,
    /// 32-bit x86.
    X86,
    /// 64-bit ARM.
    Aarch64,
    /// 32-bit ARM.
    Arm,
    /// 64-bit RISC-V.
    Riscv64,
    /// 64-bit PowerPC.
    Powerpc64,
    /// IBM Z.
    S390x,
    /// 64-bit LoongArch.
    Loongarch64,
    /// 32-bit WebAssembly.
    Wasm32,
    /// Anything else.
    Other,
}

impl Arch {
    /// The name rustc uses: `"x86_64"`, `"aarch64"`.
    ///
    /// ```
    /// assert_eq!(sys_lib::cpu::Arch::Aarch64.name(), "aarch64");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::X86 => "x86",
            Self::Aarch64 => "aarch64",
            Self::Arm => "arm",
            Self::Riscv64 => "riscv64",
            Self::Powerpc64 => "powerpc64",
            Self::S390x => "s390x",
            Self::Loongarch64 => "loongarch64",
            Self::Wasm32 => "wasm32",
            Self::Other => "other",
        }
    }

    /// Whether the architecture is 64-bit.
    ///
    /// ```
    /// // The program's own bitness, as a compile-time constant:
    /// const BUILD_IS_64: bool = sys_lib::cpu::arch().is_64bit();
    /// # let _ = BUILD_IS_64;
    /// ```
    #[must_use]
    pub const fn is_64bit(self) -> bool {
        matches!(
            self,
            Self::X86_64
                | Self::Aarch64
                | Self::Riscv64
                | Self::Powerpc64
                | Self::S390x
                | Self::Loongarch64
        )
    }
}

/// The architecture this program was compiled for.
#[must_use]
pub const fn arch() -> Arch {
    if cfg!(target_arch = "x86_64") {
        Arch::X86_64
    } else if cfg!(target_arch = "x86") {
        Arch::X86
    } else if cfg!(target_arch = "aarch64") {
        Arch::Aarch64
    } else if cfg!(target_arch = "arm") {
        Arch::Arm
    } else if cfg!(target_arch = "riscv64") {
        Arch::Riscv64
    } else if cfg!(target_arch = "powerpc64") {
        Arch::Powerpc64
    } else if cfg!(target_arch = "s390x") {
        Arch::S390x
    } else if cfg!(target_arch = "loongarch64") {
        Arch::Loongarch64
    } else if cfg!(target_arch = "wasm32") {
        Arch::Wasm32
    } else {
        Arch::Other
    }
}

/// Whether the **processor** is 64-bit. A 32-bit program on a 64-bit machine
/// still sees `true` (D-024); the program's own bitness is
/// `cpu::arch().is_64bit()`.
///
/// ```
/// assert_ne!(sys_lib::cpu::is_64bit(), sys_lib::cpu::is_32bit());
/// ```
#[must_use]
pub fn is_64bit() -> bool {
    static IS_64: OnceLock<bool> = OnceLock::new();
    *IS_64.get_or_init(crate::sys::processor_is_64bit)
}

/// Whether the processor is 32-bit only.
#[must_use]
pub fn is_32bit() -> bool {
    !is_64bit()
}

/// A CPU designer or manufacturer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum Vendor {
    /// Intel.
    Intel,
    /// AMD.
    Amd,
    /// Apple.
    Apple,
    /// Arm (Cortex and Neoverse cores).
    Arm,
    /// Qualcomm.
    Qualcomm,
    /// Ampere Computing.
    Ampere,
    /// NVIDIA.
    Nvidia,
    /// Samsung.
    Samsung,
    /// MediaTek.
    Mediatek,
    /// Hygon.
    Hygon,
    /// Zhaoxin and VIA (Centaur).
    Zhaoxin,
    /// Broadcom.
    Broadcom,
    /// Marvell (formerly Cavium).
    Marvell,
    /// Fujitsu.
    Fujitsu,
    /// HiSilicon.
    HiSilicon,
    /// Microsoft (Azure Cobalt).
    Microsoft,
    /// Not known.
    #[default]
    Unknown,
}

impl Vendor {
    /// The vendor's name: "Intel", "AMD", "ARM".
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Intel => "Intel",
            Self::Amd => "AMD",
            Self::Apple => "Apple",
            Self::Arm => "ARM",
            Self::Qualcomm => "Qualcomm",
            Self::Ampere => "Ampere",
            Self::Nvidia => "NVIDIA",
            Self::Samsung => "Samsung",
            Self::Mediatek => "MediaTek",
            Self::Hygon => "Hygon",
            Self::Zhaoxin => "Zhaoxin",
            Self::Broadcom => "Broadcom",
            Self::Marvell => "Marvell",
            Self::Fujitsu => "Fujitsu",
            Self::HiSilicon => "HiSilicon",
            Self::Microsoft => "Microsoft",
            Self::Unknown => "unknown",
        }
    }
}

struct Static {
    logical: usize,
    physical: usize,
    physical_exact: bool,
    packages: usize,
    name: &'static str,
    raw_name: Option<&'static str>,
    vendor: Vendor,
}

fn facts() -> &'static Static {
    static FACTS: OnceLock<Static> = OnceLock::new();
    FACTS.get_or_init(|| build(crate::sys::cpu()))
}

fn build(f: crate::sys::CpuFacts) -> Static {
    let logical = f.logical.max(1);
    let physical = f.physical.unwrap_or(logical).clamp(1, logical);
    let packages = f.packages.unwrap_or(1).clamp(1, physical);
    let name = f
        .name
        .or_else(|| f.raw_name.as_deref().map(cpu_name::clean))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| match f.vendor {
            Vendor::Unknown => "unknown".to_owned(),
            v => format!("{} {}", v.name(), arch().name()),
        });
    Static {
        logical,
        physical,
        physical_exact: f.physical.is_some(),
        packages,
        name: keep(name),
        raw_name: f.raw_name.filter(|n| !n.trim().is_empty()).map(keep),
        vendor: f.vendor,
    }
}

/// Configured hardware threads: the right size for per-CPU arrays. A static
/// fact (E-15: present CPUs, not the `possible` count VMs inflate). Windows
/// counts every processor group (FR-12). At least 1.
#[must_use]
pub fn logical() -> usize {
    facts().logical
}

/// Physical cores; [`logical`] when the topology is hidden (E-18). Always
/// between 1 and `logical()`.
#[must_use]
pub fn physical() -> usize {
    facts().physical
}

/// The CPUs this process may use right now: the affinity mask, intersected
/// with the cgroup cpuset, then limited by the cgroup CPU quota (Linux), or the
/// group affinity and the job object's CPU rate (Windows). Size worker pools by
/// this. A live reading: affinity, hot-plug, and quotas change. Always between
/// 1 and `logical()`.
#[must_use]
pub fn available() -> usize {
    crate::sys::cpu_available(logical()).clamp(1, logical())
}

/// The cleaned product name: "AMD Ryzen 9 7950X", "Apple M2 Pro",
/// "Cortex-A76 + Cortex-A55". Never empty: falls back to the vendor and
/// architecture ("ARM aarch64"), then "unknown".
#[must_use]
pub fn name() -> &'static str {
    facts().name
}

/// The name exactly as the OS reports it, for support output; `None` where the
/// OS reports no brand string (most ARM Linux systems).
#[must_use]
pub fn raw_name() -> Option<&'static str> {
    facts().raw_name
}

/// The vendor.
#[must_use]
pub fn vendor() -> Vendor {
    facts().vendor
}

/// Processor packages (sockets): a static fact, 1 when the OS does not say.
/// Always between 1 and `physical()`.
#[must_use]
pub fn packages() -> usize {
    facts().packages
}

/// The CPUs online now, which hot-plug changes; between 1 and `logical()`.
/// Linux: `/sys/devices/system/cpu/online`; macOS: `hw.activecpu`; Windows:
/// the active processors across every processor group.
#[must_use]
pub fn online() -> usize {
    crate::sys::cpu_online(logical()).clamp(1, logical())
}

/// Every count at once, with whether `physical` is exact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Counts {
    /// [`logical`].
    pub logical: usize,
    /// [`physical`].
    pub physical: usize,
    /// [`packages`].
    pub packages: usize,
    /// [`available`], read now.
    pub available: usize,
    /// [`online`], read now.
    pub online: usize,
    /// False when the topology was hidden and `physical` fell back to
    /// `logical` (E-18).
    pub physical_exact: bool,
}

/// Every count at once; `available` and `online` are read now.
///
/// ```
/// let c = sys_lib::cpu::count();
/// assert!(c.available <= c.logical && c.physical <= c.logical);
/// ```
#[must_use]
pub fn count() -> Counts {
    let f = facts();
    Counts {
        logical: f.logical,
        physical: f.physical,
        packages: f.packages,
        available: available(),
        online: online(),
        physical_exact: f.physical_exact,
    }
}

/// A CPU quota: how much CPU time this process may use, in CPUs.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct CpuQuota {
    /// The quota in CPUs: 1.5 is one and a half CPUs' worth of time.
    pub cpus: f32,
    /// Where it came from.
    pub source: LimitSource,
}

/// The CPU quota on this process, or `None` when there is none. Linux: the
/// smallest cgroup v2 `cpu.max` over the cgroup and its ancestors, or cgroup
/// v1's `cpu.cfs_quota_us` over `cpu.cfs_period_us`. Windows: the job object's
/// CPU rate cap. macOS: `None`. [`available`] already applies it, rounded up.
///
/// ```
/// if let Some(q) = sys_lib::cpu::quota()? {
///     println!("CPU quota {:.1} from {:?}", q.cpus, q.source);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Malformed` when a quota file does not parse; `Os` when the job-object
/// query fails.
pub fn quota() -> Result<Option<CpuQuota>> {
    crate::sys::cpu_quota()
}

/// The base and maximum clock frequency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Frequency {
    /// The base (nominal) frequency, when reported.
    pub base: Option<Mhz>,
    /// The maximum (boost) frequency, when reported.
    pub max: Option<Mhz>,
}

/// The base and maximum frequency: a static fact. Apple silicon reports
/// neither. There is no "total frequency": a sum of per-core frequencies means
/// nothing.
///
/// ```
/// if let Some(max) = sys_lib::cpu::frequency().max { println!("up to {max}"); }
/// ```
#[must_use]
pub fn frequency() -> &'static Frequency {
    static FREQUENCY: OnceLock<Frequency> = OnceLock::new();
    FREQUENCY.get_or_init(crate::sys::frequency)
}

/// The current frequency, averaged over the online CPUs. `None` where the OS
/// does not report it (macOS).
///
/// # Errors
///
/// `Malformed` when a frequency file does not parse; `Os` when the power query
/// fails.
pub fn current_frequency() -> Result<Option<Mhz>> {
    crate::sys::current_frequency()
}

/// The load average: how many programs were running or waiting for a CPU, on
/// average, over the last 1, 5, and 15 minutes.
///
/// Compare it with the number of CPUs, or use [`per_cpu`](Load::per_cpu):
/// under 1.0 per CPU there was spare capacity, over 1.0 programs were queueing.
/// On Linux it also counts programs waiting on disk, so high load with low CPU
/// usage points at storage.
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Load {
    /// The last minute.
    pub one: f32,
    /// The last 5 minutes.
    pub five: f32,
    /// The last 15 minutes.
    pub fifteen: f32,
}

impl Load {
    pub(crate) fn new(one: f32, five: f32, fifteen: f32) -> Load {
        let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
        Load {
            one: clean(one),
            five: clean(five),
            fifteen: clean(fifteen),
        }
    }

    /// Divided by [`logical`]: 1.0 means every CPU had work.
    ///
    /// ```
    /// if let Some(load) = sys_lib::cpu::load()? {
    ///     if load.per_cpu().one > 1.0 { /* programs are queueing for a CPU */ }
    /// }
    /// # Ok::<(), sys_lib::Error>(())
    /// ```
    #[must_use]
    pub fn per_cpu(&self) -> Load {
        let n = logical() as f32;
        Load::new(self.one / n, self.five / n, self.fifteen / n)
    }
}

impl fmt::Display for Load {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2} {:.2} {:.2}", self.one, self.five, self.fifteen)
    }
}

/// The load average, or `None` on Windows, which keeps none (nothing is
/// simulated). Inside a container it is the host's (with LXCFS, the
/// container's).
///
/// ```
/// match sys_lib::cpu::load()? {
///     Some(load) => println!("load {load} ({:.2} per CPU)", load.per_cpu().one),
///     None => println!("this OS keeps no load average"),
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Malformed` when `/proc/loadavg` does not parse.
pub fn load() -> Result<Option<Load>> {
    crate::sys::load()
}

/// How much runnable work waited for a CPU over the last 10, 60, and 300
/// seconds ([`Pressure`]): the direct form of what the load average
/// approximates. Linux: the cgroup's `cpu.pressure`, else `/proc/pressure/cpu`;
/// `None` on kernels without pressure-stall information. macOS and Windows
/// have no stall measure: `None`.
///
/// # Errors
///
/// `Malformed` when the kernel's file does not parse.
pub fn pressure() -> Result<Option<Pressure>> {
    crate::sys::cpu_pressure()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_with(
        logical: usize,
        physical: Option<usize>,
        raw: Option<&str>,
        name: Option<&str>,
        vendor: Vendor,
    ) -> Static {
        build(crate::sys::CpuFacts {
            logical,
            physical,
            packages: None,
            raw_name: raw.map(str::to_owned),
            name: name.map(str::to_owned),
            vendor,
        })
    }

    #[test]
    fn test_cpu_counts_hold_their_invariants() {
        assert!(physical() >= 1 && physical() <= logical());
        let a = available();
        assert!(a >= 1 && a <= logical());
        assert_ne!(is_64bit(), is_32bit());
        if arch().is_64bit() {
            assert!(is_64bit());
        }
        assert!(!name().is_empty());
    }

    #[test]
    fn test_cpu_build_cleans_and_falls_back() {
        let s = facts_with(
            8,
            Some(4),
            Some("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz"),
            None,
            Vendor::Intel,
        );
        assert_eq!(
            (s.logical, s.physical, s.name),
            (8, 4, "Intel Core i7-8700")
        );
        assert_eq!(s.raw_name, Some("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz"));
        let s = facts_with(0, Some(99), None, None, Vendor::Arm);
        assert_eq!((s.logical, s.physical), (1, 1));
        assert_eq!(s.name, format!("ARM {}", arch().name()));
        assert_eq!(
            facts_with(4, None, Some("  "), None, Vendor::Unknown).name,
            "unknown"
        );
        assert_eq!(
            facts_with(4, None, None, Some("Cortex-A76"), Vendor::Arm).name,
            "Cortex-A76"
        );
    }

    #[test]
    fn test_cpu_counts_and_load_live() {
        let c = count();
        assert!(c.packages >= 1 && c.packages <= c.physical);
        assert!(c.online >= 1 && c.online <= c.logical);
        if let Some(load) = load().unwrap() {
            assert!(load.one >= 0.0 && load.five >= 0.0 && load.fifteen >= 0.0);
        }
        let f = frequency();
        if let (Some(b), Some(m)) = (f.base, f.max) {
            assert!(b.0 > 0 && m.0 > 0);
        }
        let _ = current_frequency().unwrap();
        let _ = quota().unwrap();
        let _ = pressure().unwrap();
    }

    #[test]
    fn test_cpu_load_display_and_cleaning() {
        let l = Load::new(2.149, f32::NAN, -1.0);
        assert_eq!(l.to_string(), "2.15 0.00 0.00");
        assert!(l.per_cpu().one <= l.one);
    }

    #[test]
    fn test_cpu_packages_clamped() {
        let s = build(crate::sys::CpuFacts {
            logical: 8,
            physical: Some(4),
            packages: Some(9),
            raw_name: None,
            name: None,
            vendor: Vendor::Amd,
        });
        assert_eq!((s.packages, s.physical_exact), (4, true));
        assert!(!facts_with(8, None, None, None, Vendor::Amd).physical_exact);
    }

    #[test]
    fn test_cpu_arch_names_and_bitness() {
        assert_eq!(Arch::X86_64.name(), "x86_64");
        assert!(Arch::X86_64.is_64bit() && !Arch::X86.is_64bit() && !Arch::Arm.is_64bit());
        assert_eq!(arch().name(), std::env::consts::ARCH);
    }
}
