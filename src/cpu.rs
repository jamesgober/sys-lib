//! The processor: architecture and bitness, name and vendor, and the three
//! counts (cores, hardware threads, and what this process may use).
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

use std::sync::OnceLock;

use crate::{os::keep, parse::cpu_name};

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
    /// The vendor's name: "Intel", "AMD", "Arm".
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
    fn test_cpu_arch_names_and_bitness() {
        assert_eq!(Arch::X86_64.name(), "x86_64");
        assert!(Arch::X86_64.is_64bit() && !Arch::X86.is_64bit() && !Arch::Arm.is_64bit());
        assert_eq!(arch().name(), std::env::consts::ARCH);
    }
}
