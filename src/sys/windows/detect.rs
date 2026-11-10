//! What the machine is: virtualization, the cloud, the form factor, the
//! firmware, and the architecture under any emulation.

use super::{ffi, firmware, power, rtl_version};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use crate::sys::x86;
use crate::{
    cpu::Arch,
    host::FirmwareKind,
    os::{self, Confidence, Detection, FormFactor},
    parse::smbios::Facts,
    sys::HardwareFacts,
    virt::{self, Cloud, Dmi, Hypervisor, Strength, Virtualization},
};

fn text_of(s: Option<&String>) -> &str {
    s.map_or("", String::as_str)
}

/// The firmware strings the detection reads; a string the table lacks is empty.
fn dmi(facts: &Facts) -> Dmi<'_> {
    let system = facts.system.as_ref();
    Dmi {
        sys_vendor: text_of(system.and_then(|s| s.vendor.as_ref())),
        product: text_of(system.and_then(|s| s.product.as_ref())),
        product_version: text_of(system.and_then(|s| s.version.as_ref())),
        board_vendor: text_of(facts.board.as_ref().and_then(|b| b.vendor.as_ref())),
        bios_vendor: text_of(facts.bios.as_ref().and_then(|b| b.vendor.as_ref())),
        asset_tag: text_of(facts.asset_tag.as_ref()),
    }
}

/// CPUID's vendor signature and whether this is Hyper-V's root partition.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn cpuid() -> (Option<[u8; 12]>, bool) {
    (x86::hypervisor_signature(), x86::is_hyperv_root())
}

/// ARM has no CPUID: firmware strings are the only source.
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn cpuid() -> (Option<[u8; 12]>, bool) {
    (None, false)
}

/// CPUID names what this machine talks to, so it wins (E-85), with two
/// exceptions: firmware names a cloud's own hypervisor, which says KVM to
/// CPUID, and it can name one CPUID has no name for.
fn choose(cpuid: Option<Hypervisor>, dmi: Option<Hypervisor>) -> Option<Hypervisor> {
    match (cpuid, dmi) {
        (
            Some(Hypervisor::Kvm),
            Some(cloud @ (Hypervisor::AmazonNitro | Hypervisor::GoogleCompute)),
        ) => Some(cloud),
        (Some(Hypervisor::Other), Some(named)) => Some(named),
        (Some(h), _) | (None, Some(h)) => Some(h),
        (None, None) => None,
    }
}

/// CPUID and SMBIOS. A root partition is a Windows host with Hyper-V, WSL 2, or
/// virtualization-based security, not a VM (E-46, E-84): its signature counts
/// for nothing, and only firmware strings can still say it is nested in a VM.
pub(crate) fn virtualization() -> Virtualization {
    let mut v = Virtualization::default();
    let (signature, root) = cpuid();
    let from_cpuid = if root {
        v.fired("CPUID Hyper-V root partition", Strength::Strong);
        None
    } else {
        signature.map(|s| {
            v.fired("CPUID hypervisor leaf", Strength::Strong);
            virt::hypervisor_from_signature(&s)
        })
    };
    let from_dmi = firmware::facts().and_then(|f| virt::hypervisor_from_dmi(&dmi(f)));
    if from_dmi.is_some() {
        v.fired("SMBIOS strings", Strength::Strong);
    }
    v.hypervisor = choose(from_cpuid, from_dmi);
    v
}

pub(crate) fn cloud() -> Option<Cloud> {
    let facts = firmware::facts()?;
    virt::cloud_from_dmi(&dmi(facts), virt::hypervisor())
}

/// The first answer of the chassis, the platform role, and a battery. Inside a
/// VM the chassis describes virtual hardware, so nothing is said (E-79).
fn form_from(vm: bool, chassis: Option<u8>, role: i32, battery: bool) -> Detection<FormFactor> {
    if vm {
        return Detection::new(FormFactor::Unknown, "virtual machine", Confidence::None);
    }
    if let Some(form) = chassis.and_then(os::form_from_chassis) {
        return Detection::new(form, "SMBIOS chassis type", Confidence::High);
    }
    // The role values are the ACPI preferred power-management profiles.
    if let Some(form) = u8::try_from(role).ok().and_then(os::form_from_pm_profile) {
        return Detection::new(form, "PowerDeterminePlatformRoleEx", Confidence::Medium);
    }
    if battery {
        return Detection::new(FormFactor::Laptop, "GetSystemPowerStatus", Confidence::Low);
    }
    Detection::new(FormFactor::Unknown, "none", Confidence::None)
}

pub(crate) fn form_factor() -> Detection<FormFactor> {
    form_from(
        virt::is_vm(),
        firmware::facts().and_then(|f| f.chassis),
        ffi::PowerDeterminePlatformRoleEx(ffi::POWER_PLATFORM_ROLE_V2),
        power::has_battery(),
    )
}

/// `GetFirmwareType`; `None` when the call fails.
pub(super) fn firmware_kind() -> Option<FirmwareKind> {
    let mut kind = 0u32;
    // SAFETY: `kind` is a writable FIRMWARE_TYPE.
    let ok = unsafe { ffi::GetFirmwareType(&raw mut kind) };
    (ok != 0).then_some(match kind {
        ffi::FIRMWARE_TYPE_UEFI => FirmwareKind::Uefi,
        ffi::FIRMWARE_TYPE_BIOS => FirmwareKind::Bios,
        _ => FirmwareKind::Unknown,
    })
}

pub(crate) fn hardware() -> HardwareFacts {
    let mut out = HardwareFacts {
        firmware_kind: firmware_kind(),
        ..HardwareFacts::default()
    };
    let Some(facts) = firmware::facts() else {
        return out;
    };
    if let Some(s) = &facts.system {
        out.model_vendor.clone_from(&s.vendor);
        out.model_name.clone_from(&s.product);
        out.model_family.clone_from(&s.family);
    }
    if let Some(b) = &facts.bios {
        out.firmware_vendor.clone_from(&b.vendor);
        out.firmware_version.clone_from(&b.version);
        out.firmware_date.clone_from(&b.date);
    }
    if let Some(b) = &facts.board {
        out.board_vendor.clone_from(&b.vendor);
        out.board_name.clone_from(&b.product);
        out.board_version.clone_from(&b.version);
    }
    out
}

/// Domain controllers are servers too: only a workstation is not one.
pub(crate) fn is_server_edition() -> Option<bool> {
    rtl_version().map(|info| info.product_type != ffi::VER_NT_WORKSTATION)
}

/// The architecture an `IMAGE_FILE_MACHINE_*` value names.
fn arch_of(machine: u16) -> Option<Arch> {
    match machine {
        ffi::IMAGE_FILE_MACHINE_AMD64 => Some(Arch::X86_64),
        ffi::IMAGE_FILE_MACHINE_I386 => Some(Arch::X86),
        ffi::IMAGE_FILE_MACHINE_ARM64 => Some(Arch::Aarch64),
        ffi::IMAGE_FILE_MACHINE_ARMNT => Some(Arch::Arm),
        _ => None,
    }
}

/// `IsWow64Process2` names the hardware for an emulated x64 or x86 process too
/// (E-89).
pub(crate) fn native_arch() -> Arch {
    let (mut process, mut native) = (0u16, 0u16);
    // SAFETY: both outputs are writable locals.
    let ok = unsafe {
        ffi::IsWow64Process2(ffi::GetCurrentProcess(), &raw mut process, &raw mut native)
    };
    let found = if ok != 0 { arch_of(native) } else { None };
    found.unwrap_or_else(crate::cpu::arch)
}

/// CPUID leaf 1 EAX.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    x86::signature()
}

/// No CPUID to read on ARM.
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::smbios::{Bios, Board, System};

    #[test]
    fn test_detect_windows_dmi_strings_from_facts() {
        let facts = Facts {
            bios: Some(Bios {
                vendor: Some("Xen".to_owned()),
                ..Bios::default()
            }),
            system: Some(System {
                vendor: Some("Microsoft Corporation".to_owned()),
                product: Some("Virtual Machine".to_owned()),
                version: Some("7.0".to_owned()),
                family: None,
            }),
            board: Some(Board {
                vendor: Some("Microsoft".to_owned()),
                ..Board::default()
            }),
            chassis: Some(3),
            asset_tag: Some("tag".to_owned()),
        };
        let d = dmi(&facts);
        assert_eq!(
            (d.sys_vendor, d.product, d.product_version),
            ("Microsoft Corporation", "Virtual Machine", "7.0")
        );
        assert_eq!(
            (d.board_vendor, d.bios_vendor, d.asset_tag),
            ("Microsoft", "Xen", "tag")
        );
        let empty = Facts::default();
        let d = dmi(&empty);
        assert_eq!((d.sys_vendor, d.asset_tag), ("", ""));
    }

    #[test]
    fn test_detect_windows_cpuid_and_firmware_combine() {
        use Hypervisor::{AmazonNitro, GoogleCompute, HyperV, Kvm, Other, Qemu, VirtualBox};
        assert_eq!(choose(None, None), None);
        assert_eq!(choose(Some(HyperV), None), Some(HyperV));
        assert_eq!(choose(None, Some(VirtualBox)), Some(VirtualBox));
        // QEMU with KVM: firmware says QEMU, CPUID is right.
        assert_eq!(choose(Some(Kvm), Some(Qemu)), Some(Kvm));
        assert_eq!(choose(Some(Kvm), Some(AmazonNitro)), Some(AmazonNitro));
        assert_eq!(choose(Some(Kvm), Some(GoogleCompute)), Some(GoogleCompute));
        assert_eq!(choose(Some(Other), Some(VirtualBox)), Some(VirtualBox));
        // Nested: the innermost hypervisor visible here (E-85).
        assert_eq!(choose(Some(HyperV), Some(Kvm)), Some(HyperV));
    }

    #[test]
    fn test_detect_windows_form_factor_order() {
        const DESKTOP: i32 = ffi::PLATFORM_ROLE_DESKTOP;
        const MOBILE: i32 = ffi::PLATFORM_ROLE_MOBILE;
        const UNSPECIFIED: i32 = ffi::PLATFORM_ROLE_UNSPECIFIED;
        // E-79: nothing is believed inside a VM.
        let d = form_from(true, Some(0x03), DESKTOP, true);
        assert_eq!(
            (d.value, d.confidence),
            (FormFactor::Unknown, Confidence::None)
        );
        // The chassis comes first.
        let d = form_from(false, Some(0x0A), DESKTOP, false);
        assert_eq!(
            (d.value, d.confidence),
            (FormFactor::Laptop, Confidence::High)
        );
        // A chassis saying nothing ("Other") goes to the role.
        let d = form_from(false, Some(0x01), MOBILE, false);
        assert_eq!(
            (d.value, d.confidence),
            (FormFactor::Laptop, Confidence::Medium)
        );
        let d = form_from(false, None, ffi::PLATFORM_ROLE_SLATE, false);
        assert_eq!(d.value, FormFactor::Tablet);
        let d = form_from(false, None, ffi::PLATFORM_ROLE_ENTERPRISE_SERVER, false);
        assert_eq!(d.value, FormFactor::Server);
        // Then a battery, then nothing.
        let d = form_from(false, None, UNSPECIFIED, true);
        assert_eq!(
            (d.value, d.confidence),
            (FormFactor::Laptop, Confidence::Low)
        );
        let d = form_from(false, None, UNSPECIFIED, false);
        assert_eq!(
            (d.value, d.confidence),
            (FormFactor::Unknown, Confidence::None)
        );
        assert_eq!(form_from(false, None, -1, false).value, FormFactor::Unknown);
    }

    #[test]
    fn test_detect_windows_machines_name_architectures() {
        assert_eq!(arch_of(ffi::IMAGE_FILE_MACHINE_AMD64), Some(Arch::X86_64));
        assert_eq!(arch_of(ffi::IMAGE_FILE_MACHINE_ARM64), Some(Arch::Aarch64));
        assert_eq!(arch_of(ffi::IMAGE_FILE_MACHINE_I386), Some(Arch::X86));
        assert_eq!(arch_of(ffi::IMAGE_FILE_MACHINE_ARMNT), Some(Arch::Arm));
        assert_eq!(arch_of(ffi::IMAGE_FILE_MACHINE_UNKNOWN), None);
    }

    #[test]
    fn test_detect_windows_live_answers_hang_together() {
        let v = virtualization();
        assert!(hardware().firmware_kind.is_some());
        assert!(is_server_edition().is_some());
        assert_eq!(native_arch(), crate::cpu::native_arch());
        if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
            assert_ne!(cpu_signature(), 0);
        }
        let f = form_factor();
        assert!(!f.source.is_empty());
        if v.hypervisor.is_some() {
            assert_eq!(f.value, FormFactor::Unknown);
        }
    }
}
