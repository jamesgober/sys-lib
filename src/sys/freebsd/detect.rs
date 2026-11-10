//! Detection: what this machine is. Virtualization from `kern.vm_guest`, CPUID
//! on x86, and the jail flag; the firmware's strings from the SMBIOS values
//! the loader copies into the kernel environment (`smbios.*`, read with
//! `kenv(2)`, no privilege needed); the boot method from `machdep.bootmethod`.

use super::{kenv_text, sysctl_string, sysctl_u64};
use crate::{
    cpu::Arch,
    host::FirmwareKind,
    os::{Confidence, Detection, FormFactor},
    sys::{HardwareFacts, TopologyFacts},
    virt::{
        Cloud, Container, Dmi, Hypervisor, Strength, Virtualization, cloud_from_dmi,
        hypervisor_from_dmi, hypervisor_from_signature,
    },
};

/// Windows Server's question: FreeBSD has no server edition.
pub(crate) fn is_server_edition() -> Option<bool> {
    None
}

/// The supported targets are 64-bit, and FreeBSD runs no translated
/// binaries: the machine is what this process was compiled for.
pub(crate) fn native_arch() -> Arch {
    crate::cpu::arch()
}

/// CPUID leaf 1 EAX on x86.
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    crate::sys::x86::signature()
}

/// arm64 has no CPUID; the brand string identifies the core.
#[cfg(not(target_arch = "x86_64"))]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    0
}

/// The layout is in `kern.sched.topology_spec`, an XML document of nested
/// groups; it is not parsed yet, so the topology is unknown (the counts come
/// from `kern.smp.cores` and `hw.ncpu`).
pub(crate) fn cpu_topology() -> TopologyFacts {
    TopologyFacts::default()
}

/// The hypervisor `kern.vm_guest` names (`sys/kern/subr_param.c`).
fn guest_hypervisor(guest: &str) -> Option<Hypervisor> {
    Some(match guest {
        "none" | "" => return None,
        "xen" => Hypervisor::Xen,
        "hv" => Hypervisor::HyperV,
        "vmware" => Hypervisor::Vmware,
        "kvm" => Hypervisor::Kvm,
        "bhyve" => Hypervisor::Bhyve,
        "vbox" => Hypervisor::VirtualBox,
        "parallels" => Hypervisor::Parallels,
        _ => Hypervisor::Other,
    })
}

/// The firmware strings, owned.
#[derive(Clone, Debug, Default)]
struct Firmware {
    sys_vendor: String,
    product: String,
    product_version: String,
    board_vendor: String,
    bios_vendor: String,
    asset_tag: String,
}

impl Firmware {
    fn read() -> Firmware {
        let text = |name| kenv_text(name).unwrap_or_default();
        Firmware {
            sys_vendor: text(c"smbios.system.maker"),
            product: text(c"smbios.system.product"),
            product_version: text(c"smbios.system.version"),
            board_vendor: text(c"smbios.planar.maker"),
            bios_vendor: text(c"smbios.bios.vendor"),
            asset_tag: text(c"smbios.chassis.tag"),
        }
    }

    fn dmi(&self) -> Dmi<'_> {
        Dmi {
            sys_vendor: &self.sys_vendor,
            product: &self.product,
            product_version: &self.product_version,
            board_vendor: &self.board_vendor,
            bios_vendor: &self.bios_vendor,
            asset_tag: &self.asset_tag,
        }
    }
}

/// The hypervisor's CPUID signature.
#[cfg(target_arch = "x86_64")]
fn cpuid_signature() -> Option<[u8; 12]> {
    crate::sys::x86::hypervisor_signature()
}

/// arm64 has no CPUID.
#[cfg(not(target_arch = "x86_64"))]
fn cpuid_signature() -> Option<[u8; 12]> {
    None
}

pub(crate) fn virtualization() -> Virtualization {
    let guest = sysctl_string(c"kern.vm_guest");
    let jailed = sysctl_u64(c"security.jail.jailed") == Some(1);
    let firmware = Firmware::read();
    virt_from(guest.as_deref(), cpuid_signature(), jailed, &firmware.dmi())
}

/// The answer from the checks. The CPUID signature names the hypervisor when
/// there is one, then the kernel's own guess (`kern.vm_guest`, which it makes
/// from the same CPUID leaf and the SMBIOS strings), then the firmware
/// strings. A jail is an OS-level container: `Container::Other`.
fn virt_from(
    guest: Option<&str>,
    signature: Option<[u8; 12]>,
    jailed: bool,
    dmi: &Dmi<'_>,
) -> Virtualization {
    let mut v = Virtualization::default();
    let from_guest = guest.and_then(guest_hypervisor);
    if from_guest.is_some() {
        v.fired("kern.vm_guest", Strength::Strong);
    }
    if signature.is_some() {
        v.fired("CPUID hypervisor leaf", Strength::Strong);
    }
    let from_dmi = hypervisor_from_dmi(dmi);
    if from_dmi.is_some() {
        v.fired("smbios strings", Strength::Weak);
    }
    v.hypervisor = match (signature, from_guest) {
        (Some(s), _) => Some(hypervisor_from_signature(&s)),
        (None, Some(h)) => Some(h),
        (None, None) => from_dmi,
    };
    if jailed {
        v.fired("security.jail.jailed", Strength::Strong);
        v.container = Some(Container::Other);
    }
    v
}

/// The cloud the firmware strings name, or the cloud's own hypervisor.
pub(crate) fn cloud() -> Option<Cloud> {
    let firmware = Firmware::read();
    let dmi = firmware.dmi();
    let hypervisor = cpuid_signature()
        .map(|s| hypervisor_from_signature(&s))
        .or_else(|| hypervisor_from_dmi(&dmi));
    cloud_from_dmi(&dmi, hypervisor)
}

/// The SMBIOS chassis type names, in code order from 0x01 (SMBIOS 3.5.0
/// section 7.4.1), as the loader may write them into `smbios.chassis.type`.
const CHASSIS_NAMES: [&str; 36] = [
    "Other",
    "Unknown",
    "Desktop",
    "Low Profile Desktop",
    "Pizza Box",
    "Mini Tower",
    "Tower",
    "Portable",
    "Laptop",
    "Notebook",
    "Hand Held",
    "Docking Station",
    "All in One",
    "Sub Notebook",
    "Space-saving",
    "Lunch Box",
    "Main Server Chassis",
    "Expansion Chassis",
    "SubChassis",
    "Bus Expansion Chassis",
    "Peripheral Chassis",
    "RAID Chassis",
    "Rack Mount Chassis",
    "Sealed-case PC",
    "Multi-system chassis",
    "Compact PCI",
    "Advanced TCA",
    "Blade",
    "Blade Enclosure",
    "Tablet",
    "Convertible",
    "Detachable",
    "IoT Gateway",
    "Embedded PC",
    "Mini PC",
    "Stick PC",
];

/// The SMBIOS chassis code a `smbios.chassis.type` value stands for: a
/// number (decimal or `0x` hex), or the type's name.
fn chassis_code(value: &str) -> Option<u8> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u8::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = value.parse::<u8>() {
        return Some(n);
    }
    let at = CHASSIS_NAMES
        .iter()
        .position(|name| name.eq_ignore_ascii_case(value))?;
    u8::try_from(at + 1).ok()
}

/// A virtual machine first (its chassis describes virtual hardware), then the
/// SMBIOS chassis type.
pub(crate) fn form_factor() -> Detection<FormFactor> {
    if crate::virt::is_vm() {
        return Detection::new(FormFactor::Unknown, "virtual machine", Confidence::Low);
    }
    form_from(kenv_text(c"smbios.chassis.type").as_deref())
}

fn form_from(chassis: Option<&str>) -> Detection<FormFactor> {
    const SOURCE: &str = "smbios.chassis.type";
    let Some(value) = chassis else {
        return Detection::new(FormFactor::Unknown, "none", Confidence::None);
    };
    match chassis_code(value).and_then(crate::os::form_from_chassis) {
        Some(form) => Detection::new(form, SOURCE, Confidence::High),
        None => Detection::new(FormFactor::Unknown, SOURCE, Confidence::Low),
    }
}

/// `machdep.bootmethod`: `UEFI` or `BIOS` (amd64; arm64 boots UEFI only and
/// has no such sysctl).
fn firmware_kind(method: Option<&str>) -> Option<FirmwareKind> {
    match method {
        Some("UEFI") => Some(FirmwareKind::Uefi),
        Some("BIOS") => Some(FirmwareKind::Bios),
        Some(_) => Some(FirmwareKind::Unknown),
        None if cfg!(target_arch = "aarch64") => Some(FirmwareKind::Uefi),
        None => None,
    }
}

/// The model, firmware, and board strings, no serial numbers.
pub(crate) fn hardware() -> HardwareFacts {
    HardwareFacts {
        model_vendor: kenv_text(c"smbios.system.maker"),
        model_name: kenv_text(c"smbios.system.product"),
        model_family: kenv_text(c"smbios.system.family"),
        firmware_kind: firmware_kind(sysctl_string(c"machdep.bootmethod").as_deref()),
        firmware_vendor: kenv_text(c"smbios.bios.vendor"),
        firmware_version: kenv_text(c"smbios.bios.version"),
        firmware_date: kenv_text(c"smbios.bios.reldate")
            .map(|d| crate::parse::smbios::iso_date(&d)),
        board_vendor: kenv_text(c"smbios.planar.maker"),
        board_name: kenv_text(c"smbios.planar.product"),
        board_version: kenv_text(c"smbios.planar.version"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_freebsd_guest_names() {
        assert_eq!(guest_hypervisor("none"), None);
        assert_eq!(guest_hypervisor("kvm"), Some(Hypervisor::Kvm));
        assert_eq!(guest_hypervisor("hv"), Some(Hypervisor::HyperV));
        assert_eq!(guest_hypervisor("bhyve"), Some(Hypervisor::Bhyve));
        assert_eq!(guest_hypervisor("generic"), Some(Hypervisor::Other));
    }

    #[test]
    fn test_detect_freebsd_virt_from_checks() {
        let none = Dmi::default();
        let bare = virt_from(Some("none"), None, false, &none);
        assert_eq!((bare.hypervisor, bare.container), (None, None));
        assert!(bare.evidence.is_empty());
        let kvm = virt_from(Some("kvm"), Some(*b"KVMKVMKVM\0\0\0"), false, &none);
        assert_eq!(kvm.hypervisor, Some(Hypervisor::Kvm));
        assert_eq!(kvm.evidence.len(), 2);
        // The CPUID signature names it before the kernel's guess.
        let nitro = virt_from(Some("generic"), Some(*b"bhyve bhyve "), false, &none);
        assert_eq!(nitro.hypervisor, Some(Hypervisor::Bhyve));
        let qemu = Dmi {
            sys_vendor: "QEMU",
            ..Dmi::default()
        };
        let tcg = virt_from(Some("none"), None, false, &qemu);
        assert_eq!(tcg.hypervisor, Some(Hypervisor::Qemu));
        let jail = virt_from(Some("none"), None, true, &none);
        assert_eq!(jail.container, Some(Container::Other));
        assert_eq!(jail.hypervisor, None);
    }

    #[test]
    fn test_detect_freebsd_chassis() {
        assert_eq!(chassis_code("3"), Some(3));
        assert_eq!(chassis_code("0x0A"), Some(10));
        assert_eq!(chassis_code("Notebook"), Some(10));
        assert_eq!(chassis_code("rack mount chassis"), Some(0x17));
        assert_eq!(chassis_code("Stick PC"), Some(0x24));
        assert_eq!(chassis_code("Spaceship"), None);
        assert_eq!(chassis_code("0xZZ"), None);
        assert_eq!(form_from(Some("Notebook")).value, FormFactor::Laptop);
        assert_eq!(form_from(Some("Desktop")).confidence, Confidence::High);
        assert_eq!(
            form_from(Some("Rack Mount Chassis")).value,
            FormFactor::Server
        );
        assert_eq!(form_from(Some("Other")).value, FormFactor::Unknown);
        assert_eq!(form_from(None).confidence, Confidence::None);
    }

    #[test]
    fn test_detect_freebsd_firmware_kind() {
        assert_eq!(firmware_kind(Some("UEFI")), Some(FirmwareKind::Uefi));
        assert_eq!(firmware_kind(Some("BIOS")), Some(FirmwareKind::Bios));
        assert_eq!(firmware_kind(Some("x")), Some(FirmwareKind::Unknown));
    }

    #[test]
    fn test_detect_freebsd_live_detection() {
        let v = virtualization();
        if v.hypervisor.is_some() {
            assert!(!v.evidence.is_empty());
        }
        let _ = cloud();
        let _ = form_factor();
        let h = hardware();
        if cfg!(target_arch = "x86_64") {
            assert!(h.firmware_kind.is_some(), "{h:?}");
        }
        assert_eq!(native_arch(), crate::cpu::arch());
        assert_eq!(is_server_edition(), None);
    }
}
