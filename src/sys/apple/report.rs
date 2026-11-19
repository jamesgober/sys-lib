//! Environment and reports (0.6): temperatures, graphics adapters, and the
//! settings the readiness, benchmark, and security reports read.
//!
//! Sources: IOKit `IOPCIDevice` and `IOAccelerator` services for the
//! graphics adapters (macOS); `kern.maxfiles`, `kern.maxfilesperproc`,
//! `kern.ipc.somaxconn`, `net.inet.ip.portrange.{first,last}`,
//! `getrlimit(RLIMIT_MEMLOCK)`, and `statfs` `f_flags` for the readiness
//! report. Everything Apple documents no unprivileged source for is `None` or
//! `Unsupported`; nothing private or undocumented is called.

use core::ffi::CStr;
use std::path::Path;

use super::ffi;
use crate::{
    Error, Result,
    bench::BenchFacts,
    disk::Disk,
    gpu::Gpu,
    readiness::{DataDirTuning, Fact, MemLock, Tuning},
    security::Posture,
    sensors::Temperature,
};

/// No documented API reads a temperature on macOS or iOS: Apple silicon's
/// sensors sit behind private HID event services and Intel's SMC keys are
/// undocumented (D-080).
pub(crate) fn temperatures() -> Result<Vec<Temperature>> {
    Err(super::unsupported("temperatures"))
}

/// iOS offers no IOKit to an app, and Metal (Objective-C) is out of reach of a
/// library without an Objective-C runtime dependency.
#[cfg(target_os = "ios")]
pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    Err(super::unsupported("graphics adapters"))
}

#[cfg(target_os = "macos")]
pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    gpu::list()
}

// ---------------------------------------------------------------------------
// The readiness report
// ---------------------------------------------------------------------------

/// An integer sysctl, its error naming it.
fn setting(what: &'static str, name: &'static CStr) -> Result<u64> {
    super::sysctl_int(name).map_err(|e| e.into_error(what, name))
}

/// A reading as a fact. On iOS a setting the sandbox refuses is left out of
/// the report (`Skip`): an app cannot change it, so it is not a finding.
fn fact<T>(r: Result<T>) -> Fact<T> {
    if cfg!(target_os = "ios") {
        match r {
            Ok(v) => Fact::Value(v),
            Err(_) => Fact::Skip,
        }
    } else {
        Fact::of(r)
    }
}

/// The ephemeral port range, when it is one: both ends are ports and the
/// first is not above the last.
fn port_range(first: u64, last: u64) -> Option<(u32, u32)> {
    let first = u16::try_from(first).ok()?;
    let last = u16::try_from(last).ok()?;
    if first == 0 || first > last {
        return None;
    }
    Some((u32::from(first), u32::from(last)))
}

fn ports() -> Result<(u32, u32)> {
    const WHAT: &str = "ephemeral ports";
    let first = setting(WHAT, c"net.inet.ip.portrange.first")?;
    let last = setting(WHAT, c"net.inet.ip.portrange.last")?;
    port_range(first, last).ok_or(Error::Malformed {
        what: WHAT,
        source: "net.inet.ip.portrange",
        line: None,
    })
}

/// A soft resource limit, `None` when it is unlimited.
fn soft_limit(cur: u64) -> Option<u64> {
    (cur < ffi::RLIM_INFINITY).then_some(cur)
}

fn memlock() -> Result<MemLock> {
    let lim = super::rlimit(ffi::RLIMIT_MEMLOCK, "locked-memory limit")?;
    Ok(MemLock::Limit {
        soft: soft_limit(lim.cur),
    })
}

/// Apple platforms have no transparent huge pages, swappiness, overcommit
/// policy, map count, write-back ratios, NUMA balancing, user-visible CPU
/// governor, or clock-source choice: those checks are left out.
pub(crate) fn tuning() -> Tuning {
    const FILES: &str = "open-file ceiling";
    Tuning {
        files_system: fact(setting(FILES, c"kern.maxfiles")),
        files_process: fact(setting(FILES, c"kern.maxfilesperproc")),
        memlock: fact(memlock()),
        // XNU has one listen queue limit; no separate SYN backlog.
        backlog: fact(setting("listen backlog", c"kern.ipc.somaxconn").map(|n| (n, None))),
        ports: fact(ports()),
        ..Tuning::default()
    }
}

/// How a mount keeps access times, from its `statfs` flags and type.
///
/// `MNT_NOATIME` is "noatime" and `MNT_STRICTATIME` "strictatime". APFS
/// without either updates an access time only when it is older than the
/// modification time (the Apple File System Reference,
/// `APFS_FEATURE_STRICTATIME`): "relatime", the nearest Linux name. Any other
/// filesystem without either is taken as "strictatime", since nothing
/// documents otherwise (D-083).
fn atime_policy(flags: u32, filesystem: &[u8]) -> &'static str {
    if flags & ffi::MNT_NOATIME != 0 {
        "noatime"
    } else if flags & ffi::MNT_STRICTATIME != 0 {
        "strictatime"
    } else if filesystem == b"apfs" {
        "relatime"
    } else {
        "strictatime"
    }
}

fn atime(path: &Path) -> Result<String> {
    let mut buf = [0u8; super::disk::PATH_MAX];
    let c_path = super::disk::c_path(path, &mut buf)?;
    let st = super::disk::statfs(c_path)
        .map_err(|code| super::os_error("data directory mount options", "statfs", code))?;
    let filesystem = super::c_array_bytes(&st.fstypename);
    Ok(atime_policy(st.flags, &filesystem).to_owned())
}

/// The I/O scheduler is not a setting on Apple platforms: left out.
pub(crate) fn data_dir_tuning(path: &Path, _disk: &Disk) -> DataDirTuning {
    DataDirTuning {
        atime: fact(atime(path)),
        scheduler: Fact::Skip,
    }
}

// ---------------------------------------------------------------------------
// The benchmark environment and the security posture
// ---------------------------------------------------------------------------

/// SMT from the core counts; ASLR is always on. Apple exposes no governor,
/// turbo switch, throttle counter, clock-source choice, or CPU isolation to
/// a user process: `None`.
pub(crate) fn bench_facts() -> BenchFacts {
    let (physical, _) = super::cpu_counts();
    let logical = super::cpu_logical();
    BenchFacts {
        smt: physical.zip(logical).map(|(p, l)| p != l),
        aslr: Some(true),
        ..BenchFacts::default()
    }
}

/// ASLR is always on (every executable is position-independent and the
/// kernel slides it). The rest is `None` (D-085, D-086):
///
/// - System Integrity Protection: `csr_get_active_config` and `csr_check` are
///   in libSystem, but their header (`sys/csr.h`) is private to XNU and not in
///   the SDK, and Apple documents neither.
/// - Secure Boot: Apple silicon's boot policy is read by `bputil` and Intel's
///   T2 policy by an NVRAM variable; neither has a documented API.
/// - TPM: Macs have none (the Secure Enclave is not a TPM).
/// - FileVault: `fdesetup` answers, but it is a program and there is no
///   documented API under it.
/// - Firewall: the application firewall's state has no documented API.
/// - SELinux, AppArmor, lockdown, CPU vulnerabilities: not on this platform.
pub(crate) fn posture() -> Posture {
    Posture {
        aslr: Some(true),
        ..Posture::default()
    }
}

// ---------------------------------------------------------------------------
// Graphics adapters (macOS)
// ---------------------------------------------------------------------------

/// Graphics adapters from the IORegistry.
///
/// A Mac with PCI graphics (every Intel Mac, and the cards in a Mac Pro) lists
/// them as `IOPCIDevice` entries with a display class code. Apple silicon's
/// GPU is not on PCI: it is an `IOAccelerator` service (`AGXAcceleratorG13X`,
/// `AGXAcceleratorG14G`, ...; the documented base class is matched, which
/// finds its subclasses), read only when no PCI adapter was found, so a PCI
/// adapter is never listed twice through its own accelerator (D-081).
#[cfg(target_os = "macos")]
mod gpu {
    use super::super::iokit::{Cf, Io, Ref};
    use crate::{
        Bytes, Result,
        gpu::{Gpu, GpuKind, GpuVendor, kind_of},
    };

    const WHAT: &str = "graphics adapters";
    /// The most PCI functions read: a Mac Pro has a few hundred.
    const MAX_PCI: usize = 4096;
    /// The most accelerators read.
    const MAX_ACCELERATORS: usize = 64;
    /// The longest name kept, in bytes.
    const MAX_NAME: usize = 256;

    /// The property names, made once per listing.
    struct Keys {
        class_code: Cf,
        model: Cf,
        vendor_id: Cf,
        device_id: Cf,
        vram_mb: Cf,
        vram_size: Cf,
        bundle: Cf,
    }

    impl Keys {
        fn new() -> Result<Keys> {
            let key = |name: &str| Cf::string(WHAT, name);
            Ok(Keys {
                class_code: key("class-code")?,
                model: key("model")?,
                vendor_id: key("vendor-id")?,
                device_id: key("device-id")?,
                vram_mb: key("VRAM,totalMB")?,
                vram_size: key("VRAM,totalsize")?,
                bundle: key("CFBundleIdentifier")?,
            })
        }
    }

    pub(super) fn list() -> Result<Vec<Gpu>> {
        let keys = Keys::new()?;
        let mut out = Vec::new();
        Io::for_each(c"IOPCIDevice", MAX_PCI, WHAT, |device| {
            if let Some(gpu) = pci(device, &keys) {
                out.push(gpu);
            }
            Ok(())
        })?;
        if out.is_empty() {
            Io::for_each(c"IOAccelerator", MAX_ACCELERATORS, WHAT, |entry| {
                out.push(accelerator(entry, &keys));
                Ok(())
            })?;
        }
        Ok(out)
    }

    /// One PCI function, when it is a display controller.
    fn pci(device: &Io, keys: &Keys) -> Option<Gpu> {
        let class = device.property(&keys.class_code)?;
        if !is_display(class.get().data()?) {
            return None;
        }
        let vendor_id = id(device, &keys.vendor_id);
        let device_id = id(device, &keys.device_id);
        let vendor = vendor_id.map_or(GpuVendor::Other, GpuVendor::from_pci);
        let vram = vram(device, keys);
        let name = device
            .property(&keys.model)
            .and_then(|model| name_of(model.get()))
            .unwrap_or_else(|| fallback_name(vendor, device_id));
        let kind = pci_kind(vendor, &name, vram.is_some());
        // An integrated adapter's figure is the share of main memory it may
        // use, not memory of its own.
        let memory = if kind == GpuKind::Integrated {
            None
        } else {
            vram
        };
        Some(Gpu {
            name,
            vendor,
            kind,
            vendor_id,
            device_id,
            memory,
            driver: None,
        })
    }

    /// One accelerator: Apple silicon's GPU, or a hypervisor's.
    fn accelerator(entry: &Io, keys: &Keys) -> Gpu {
        let class = entry.class().unwrap_or_default();
        let apple_gpu = class.starts_with(b"AGX");
        let vendor_id = id(entry, &keys.vendor_id);
        let device_id = id(entry, &keys.device_id);
        let vendor = match vendor_id {
            Some(v) => GpuVendor::from_pci(v),
            None if apple_gpu || class.starts_with(b"Apple") => GpuVendor::Apple,
            None => GpuVendor::Other,
        };
        let name = entry
            .property(&keys.model)
            .and_then(|model| name_of(model.get()))
            .or_else(|| {
                // The GPU is part of the chip, which is named "Apple M2".
                if apple_gpu {
                    super::super::sysctl_string(c"machdep.cpu.brand_string")
                } else {
                    None
                }
            })
            .or_else(|| {
                let class = crate::parse::lossy(&class, MAX_NAME);
                if class.is_empty() { None } else { Some(class) }
            })
            .unwrap_or_else(|| fallback_name(vendor, device_id));
        let driver = entry
            .property(&keys.bundle)
            .and_then(|bundle| name_of(bundle.get()));
        Gpu {
            name,
            vendor,
            kind: accelerator_kind(&class, vendor),
            vendor_id,
            device_id,
            // Apple silicon's memory is shared with the CPU.
            memory: None,
            driver,
        }
    }

    /// A PCI adapter's kind. Apple makes no PCI graphics: an Apple display
    /// controller on PCI is the paravirtualised one a macOS virtual machine
    /// on an Intel host shows ("Apple Paravirtualized Graphics Device"). AMD
    /// makes both kinds; memory of its own makes it a card.
    pub(super) fn pci_kind(vendor: GpuVendor, name: &str, own_memory: bool) -> GpuKind {
        if vendor == GpuVendor::Apple || name.contains("Paravirtual") {
            return GpuKind::Virtual;
        }
        match kind_of(vendor) {
            GpuKind::Unknown if own_memory => GpuKind::Discrete,
            kind => kind,
        }
    }

    /// An accelerator's kind from its class: Apple's own GPU is integrated, a
    /// paravirtualised one (`AppleParavirtGPU`, in a macOS virtual machine)
    /// virtual.
    pub(super) fn accelerator_kind(class: &[u8], vendor: GpuVendor) -> GpuKind {
        if class.starts_with(b"AGX") {
            GpuKind::Integrated
        } else if class.windows(8).any(|w| w == b"Paravirt") {
            GpuKind::Virtual
        } else {
            kind_of(vendor)
        }
    }

    /// "AMD GPU 0x7340", for an adapter without a `model`.
    fn fallback_name(vendor: GpuVendor, device_id: Option<u16>) -> String {
        match device_id {
            Some(id) => format!("{} GPU {id:#06x}", vendor.name()),
            None => format!("{} GPU", vendor.name()),
        }
    }

    /// A PCI id property.
    fn id(entry: &Io, key: &Cf) -> Option<u16> {
        let property = entry.property(key)?;
        pci_id(property.get().data()?)
    }

    /// Dedicated memory: `VRAM,totalMB` (a number of MiB), else
    /// `VRAM,totalsize` (bytes, as data or a number).
    fn vram(device: &Io, keys: &Keys) -> Option<Bytes> {
        let mib = device
            .property(&keys.vram_mb)
            .and_then(|n| n.get().int())
            .and_then(|n| u64::try_from(n).ok())
            .filter(|&n| n > 0)
            .and_then(|n| n.checked_mul(1 << 20));
        let bytes = mib.or_else(|| {
            let size = device.property(&keys.vram_size)?;
            let size = size.get();
            size.int()
                .and_then(|n| u64::try_from(n).ok())
                .or_else(|| size.data().and_then(le_size))
                .filter(|&n| n > 0)
        });
        bytes.map(Bytes::new)
    }

    /// A name property: a `CFString`, or data holding NUL-terminated text
    /// (PCI devices' `model`).
    fn name_of(value: Ref<'_>) -> Option<String> {
        let mut buf = [0u8; MAX_NAME];
        let text: &[u8] = match value.text(&mut buf) {
            Some(text) => text,
            None => {
                let data = value.data()?;
                let data = &data[..data.len().min(MAX_NAME)];
                data.split(|&b| b == 0).next().unwrap_or(data)
            }
        };
        let name = crate::parse::lossy(crate::parse::trim(text), MAX_NAME);
        if name.is_empty() { None } else { Some(name) }
    }

    /// The class code of a PCI function (`class-code`, a little-endian 32-bit
    /// value 0x00BBSSPP) names a display controller (base class 0x03: VGA,
    /// XGA, 3D, or other).
    pub(super) fn is_display(class_code: &[u8]) -> bool {
        matches!(class_code, [_, _, 0x03, 0x00])
    }

    /// A PCI id (`vendor-id`, `device-id`): little-endian, 16 bits or 16 bits
    /// zero-extended to 32. 0 and 0xffff are not ids.
    pub(super) fn pci_id(data: &[u8]) -> Option<u16> {
        let id = match *data {
            [lo, hi] | [lo, hi, 0, 0] => u16::from_le_bytes([lo, hi]),
            _ => return None,
        };
        (id != 0 && id != 0xffff).then_some(id)
    }

    /// A little-endian size of 4 or 8 bytes.
    pub(super) fn le_size(data: &[u8]) -> Option<u64> {
        match *data {
            [a, b, c, d] => Some(u64::from(u32::from_le_bytes([a, b, c, d]))),
            [a, b, c, d, e, f, g, h] => Some(u64::from_le_bytes([a, b, c, d, e, f, g, h])),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_apple_port_range() {
        assert_eq!(port_range(49_152, 65_535), Some((49_152, 65_535)));
        assert_eq!(port_range(1024, 1024), Some((1024, 1024)));
        assert_eq!(port_range(0, 65_535), None);
        assert_eq!(port_range(60_000, 50_000), None);
        assert_eq!(port_range(49_152, 70_000), None);
        assert_eq!(port_range(u64::MAX, 1), None);
    }

    #[test]
    fn test_report_apple_soft_limit_infinity_is_none() {
        assert_eq!(soft_limit(ffi::RLIM_INFINITY), None);
        assert_eq!(soft_limit(u64::MAX), None);
        assert_eq!(soft_limit(65_536), Some(65_536));
        assert_eq!(soft_limit(0), Some(0));
    }

    #[test]
    fn test_report_apple_atime_policy_from_flags() {
        const LOCAL: u32 = ffi::MNT_LOCAL;
        assert_eq!(atime_policy(LOCAL | ffi::MNT_NOATIME, b"apfs"), "noatime");
        assert_eq!(
            atime_policy(LOCAL | ffi::MNT_STRICTATIME, b"apfs"),
            "strictatime"
        );
        // noatime wins: nothing is updated at all.
        assert_eq!(
            atime_policy(ffi::MNT_NOATIME | ffi::MNT_STRICTATIME, b"apfs"),
            "noatime"
        );
        assert_eq!(atime_policy(LOCAL, b"apfs"), "relatime");
        assert_eq!(atime_policy(LOCAL, b"hfs"), "strictatime");
        assert_eq!(atime_policy(0, b"smbfs"), "strictatime");
        assert_eq!(atime_policy(ffi::MNT_NOATIME, b"msdos"), "noatime");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_report_apple_pci_class_code_is_display() {
        // VGA 0x030000, 3D 0x030200, other display 0x038000, little-endian.
        assert!(gpu::is_display(&[0x00, 0x00, 0x03, 0x00]));
        assert!(gpu::is_display(&[0x00, 0x02, 0x03, 0x00]));
        assert!(gpu::is_display(&[0x00, 0x80, 0x03, 0x00]));
        // Ethernet 0x020000, NVMe 0x010802, big-endian VGA, wrong lengths.
        assert!(!gpu::is_display(&[0x00, 0x00, 0x02, 0x00]));
        assert!(!gpu::is_display(&[0x02, 0x08, 0x01, 0x00]));
        assert!(!gpu::is_display(&[0x00, 0x03, 0x00, 0x00]));
        assert!(!gpu::is_display(&[0x00, 0x00, 0x03]));
        assert!(!gpu::is_display(&[]));
        assert!(!gpu::is_display(&[0x00, 0x00, 0x03, 0x00, 0x00]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_report_apple_pci_id_little_endian() {
        assert_eq!(gpu::pci_id(&[0xde, 0x10, 0x00, 0x00]), Some(0x10de));
        assert_eq!(gpu::pci_id(&[0x6b, 0x10, 0x00, 0x00]), Some(0x106b));
        assert_eq!(gpu::pci_id(&[0x02, 0x10]), Some(0x1002));
        assert_eq!(gpu::pci_id(&[0x86, 0x80, 0x01, 0x00]), None);
        assert_eq!(gpu::pci_id(&[0xff, 0xff, 0x00, 0x00]), None);
        assert_eq!(gpu::pci_id(&[0x00, 0x00, 0x00, 0x00]), None);
        assert_eq!(gpu::pci_id(&[0x86]), None);
        assert_eq!(gpu::pci_id(&[]), None);
        assert_eq!(gpu::le_size(&[0x00, 0x00, 0x00, 0x20]), Some(0x2000_0000));
        assert_eq!(gpu::le_size(&[0, 0, 0, 0, 1, 0, 0, 0]), Some(1 << 32));
        assert_eq!(gpu::le_size(&[1, 2, 3]), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_report_apple_pci_kind() {
        use crate::gpu::{GpuKind, GpuVendor};
        // What the macos-15-intel runner (a virtual machine) lists.
        let paravirt = "Apple Paravirtualized Graphics Device";
        assert_eq!(
            gpu::pci_kind(GpuVendor::Apple, paravirt, false),
            GpuKind::Virtual
        );
        assert_eq!(
            gpu::pci_kind(GpuVendor::Other, paravirt, false),
            GpuKind::Virtual
        );
        assert_eq!(
            gpu::pci_kind(GpuVendor::Intel, "Intel UHD Graphics 630", false),
            GpuKind::Integrated
        );
        assert_eq!(
            gpu::pci_kind(GpuVendor::Amd, "AMD Radeon Pro 5500M", true),
            GpuKind::Discrete
        );
        assert_eq!(
            gpu::pci_kind(GpuVendor::Amd, "AMD GPU 0x7340", false),
            GpuKind::Unknown
        );
        assert_eq!(
            gpu::pci_kind(GpuVendor::Nvidia, "NVIDIA GeForce GT 750M", false),
            GpuKind::Discrete
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_report_apple_accelerator_kind() {
        use crate::gpu::{GpuKind, GpuVendor};
        assert_eq!(
            gpu::accelerator_kind(b"AGXAcceleratorG13X", GpuVendor::Apple),
            GpuKind::Integrated
        );
        assert_eq!(
            gpu::accelerator_kind(b"AppleParavirtGPU", GpuVendor::Apple),
            GpuKind::Virtual
        );
        assert_eq!(
            gpu::accelerator_kind(b"IntelAccelerator", GpuVendor::Intel),
            GpuKind::Integrated
        );
        assert_eq!(
            gpu::accelerator_kind(b"", GpuVendor::Other),
            GpuKind::Unknown
        );
    }

    #[test]
    fn test_report_apple_live_tuning() {
        let t = tuning();
        if cfg!(target_os = "macos") {
            assert!(matches!(t.files_system, Fact::Value(n) if n > 0), "{t:?}");
            assert!(matches!(t.files_process, Fact::Value(n) if n > 0), "{t:?}");
            assert!(
                matches!(t.memlock, Fact::Value(MemLock::Limit { .. })),
                "{t:?}"
            );
            assert!(
                matches!(t.backlog, Fact::Value((n, None)) if n > 0),
                "{t:?}"
            );
            assert!(matches!(t.ports, Fact::Value((a, b)) if a <= b), "{t:?}");
        }
        assert_eq!(t.thp, Fact::Skip);
        assert_eq!(t.governor, Fact::Skip);
        assert_eq!(t.clock_source, Fact::Skip);
        assert!(!t.handles_unlimited);
    }

    #[test]
    fn test_report_apple_live_data_dir_and_reports() {
        let dir = std::env::temp_dir();
        let disk = crate::disk::of(&dir).unwrap();
        let d = data_dir_tuning(&dir, &disk);
        if cfg!(target_os = "macos") {
            let known = ["noatime", "relatime", "strictatime"];
            assert!(
                matches!(&d.atime, Fact::Value(a) if known.contains(&a.as_str())),
                "{d:?}"
            );
        }
        assert_eq!(d.scheduler, Fact::Skip);
        let b = bench_facts();
        assert_eq!(b.aslr, Some(true));
        assert!(b.smt.is_some());
        assert_eq!(b.governor, None);
        let p = posture();
        assert_eq!(p.aslr, Some(true));
        assert_eq!(p.elevated, None);
        assert_eq!(p.code_integrity, None);
        assert!(temperatures().unwrap_err().is_unavailable());
    }

    #[test]
    fn test_report_apple_live_gpus() {
        match gpus() {
            Ok(list) => {
                for g in list {
                    assert!(!g.name.is_empty(), "{g:?}");
                    if g.kind == crate::gpu::GpuKind::Integrated {
                        assert_eq!(g.memory, None, "{g:?}");
                    }
                }
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
    }
}
