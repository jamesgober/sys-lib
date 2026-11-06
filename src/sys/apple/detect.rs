//! Detection: what this machine is. The native architecture, virtualization,
//! the form factor, and the firmware's model strings, from sysctls, CPUID on
//! Intel, and (macOS) the IORegistry. On iOS the model comes from `uname`.

#[cfg(target_os = "ios")]
use core::mem::MaybeUninit;

#[cfg(target_os = "ios")]
use super::ffi;
#[cfg(target_os = "macos")]
use super::iokit::{Cf, Io};
use super::{sysctl_string, sysctl_u64};
#[cfg(target_os = "macos")]
use crate::host::FirmwareKind;
use crate::{
    cpu::Arch,
    os::{Confidence, Detection, FormFactor},
    sys::HardwareFacts,
    virt::{Cloud, Hypervisor, Strength, Virtualization, hypervisor_from_signature},
};

/// The maker every Apple machine reports.
const APPLE: &str = "Apple Inc.";
/// The prefix of the model identifier of a macOS virtual machine.
const VIRTUAL_MAC: &str = "VirtualMac";

/// Windows Server's question: macOS has no server edition.
pub(crate) fn is_server_edition() -> Option<bool> {
    None
}

/// `sysctl.proc_translated`: 1 under Rosetta 2, 0 or absent otherwise.
fn translated() -> bool {
    sysctl_u64(c"sysctl.proc_translated") == Some(1)
}

/// `hw.optional.arm64` is 1 on Apple silicon, for translated processes too.
pub(crate) fn native_arch() -> Arch {
    native_from(
        crate::cpu::arch(),
        translated(),
        sysctl_u64(c"hw.optional.arm64") == Some(1),
    )
}

/// What the machine is, given what this process was compiled for.
fn native_from(compiled: Arch, translated: bool, arm64: bool) -> Arch {
    if translated || arm64 {
        Arch::Aarch64
    } else {
        compiled
    }
}

/// CPUID leaf 1 EAX on Intel.
#[cfg(target_arch = "x86_64")]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    crate::sys::x86::signature()
}

/// Apple silicon has no CPUID; the brand string identifies the chip.
#[cfg(not(target_arch = "x86_64"))]
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn cpu_signature() -> u32 {
    0
}

/// The hypervisor's CPUID signature. Rosetta 2 answers CPUID too, and
/// translation is not virtualization (`cpu::is_translated()` reports it).
#[cfg(target_arch = "x86_64")]
fn cpuid_signature() -> Option<[u8; 12]> {
    if translated() {
        None
    } else {
        crate::sys::x86::hypervisor_signature()
    }
}

/// Apple silicon has no CPUID.
#[cfg(not(target_arch = "x86_64"))]
fn cpuid_signature() -> Option<[u8; 12]> {
    None
}

/// `kern.hv_vmm_present`, the model identifier, and CPUID on Intel. Macs have
/// no containers to detect.
pub(crate) fn virtualization() -> Virtualization {
    virt_from(
        sysctl_u64(c"kern.hv_vmm_present") == Some(1),
        sysctl_string(c"hw.model").as_deref(),
        cpuid_signature(),
    )
}

/// The answer from the three checks. The CPUID signature names the hypervisor
/// when there is one; on Apple silicon only the model identifier can, and
/// Parallels guests use that identifier too, because they run on Apple's
/// framework.
fn virt_from(vmm: bool, model: Option<&str>, signature: Option<[u8; 12]>) -> Virtualization {
    let mut v = Virtualization::default();
    if vmm {
        v.fired("kern.hv_vmm_present", Strength::Strong);
    }
    let virtual_mac = model.is_some_and(|m| m.starts_with(VIRTUAL_MAC));
    if virtual_mac {
        v.fired("hw.model VirtualMac", Strength::Strong);
    }
    if signature.is_some() {
        v.fired("CPUID hypervisor leaf", Strength::Strong);
    }
    v.hypervisor = match signature {
        Some(s) => Some(hypervisor_from_signature(&s)),
        None if virtual_mac => Some(Hypervisor::AppleVirtualization),
        None if vmm => Some(Hypervisor::Other),
        None => None,
    };
    v
}

/// Firmware strings are what names a cloud, and a Mac has none to read.
pub(crate) fn cloud() -> Option<Cloud> {
    None
}

/// The form a model identifier names. Apple's `Mac14,2` style names no product
/// line, but every Mac laptop has an internal battery, so one the power
/// sources say has none is a desktop; when they could not be read it stays
/// unknown.
#[cfg(any(target_os = "macos", test))]
fn form_from_model(model: &str, battery: Option<bool>) -> Detection<FormFactor> {
    const SOURCE: &str = "hw.model";
    const DESKTOPS: [&str; 4] = ["iMac", "Macmini", "MacPro", "MacStudio"];
    if model.starts_with("MacBook") {
        return Detection::new(FormFactor::Laptop, SOURCE, Confidence::High);
    }
    if DESKTOPS.iter().any(|p| model.starts_with(p)) {
        return Detection::new(FormFactor::Desktop, SOURCE, Confidence::High);
    }
    let numbered = model
        .strip_prefix("Mac")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if numbered && battery == Some(false) {
        return Detection::new(FormFactor::Desktop, SOURCE, Confidence::Medium);
    }
    Detection::new(FormFactor::Unknown, SOURCE, Confidence::Low)
}

/// A virtual machine first (its model and any battery it shows describe virtual
/// hardware), then an internal battery, then the model identifier.
#[cfg(target_os = "macos")]
pub(crate) fn form_factor() -> Detection<FormFactor> {
    if crate::virt::is_vm() {
        return Detection::new(FormFactor::Unknown, "virtual machine", Confidence::Low);
    }
    let battery = super::power::internal_battery();
    if battery == Some(true) {
        return Detection::new(FormFactor::Laptop, "IOKit power sources", Confidence::High);
    }
    match sysctl_string(c"hw.model") {
        Some(model) => form_from_model(&model, battery),
        None => Detection::new(FormFactor::Unknown, "none", Confidence::None),
    }
}

/// The form a device's machine identifier names (`iPhone15,2`).
#[cfg(any(target_os = "ios", test))]
fn form_from_machine(machine: &str) -> Option<FormFactor> {
    const FORMS: [(&str, FormFactor); 5] = [
        ("iPhone", FormFactor::Phone),
        ("iPad", FormFactor::Tablet),
        ("iPod", FormFactor::Handheld),
        ("Watch", FormFactor::Wearable),
        ("AppleTV", FormFactor::Tv),
    ];
    let form = FORMS.iter().find(|(prefix, _)| machine.starts_with(prefix));
    form.map(|&(_, form)| form)
}

/// The identifier to read and where it came from. On the simulator the machine
/// is the host's CPU, and `SIMULATOR_MODEL_IDENTIFIER` names the simulated
/// device (E-91). A real device's own identifier is never overridden by the
/// environment.
#[cfg(any(target_os = "ios", test))]
fn device_identifier<'a>(
    machine: &'a str,
    simulated: Option<&'a str>,
) -> Option<(&'a str, &'static str)> {
    if matches!(machine, "arm64" | "x86_64") {
        simulated.map(|m| (m, "SIMULATOR_MODEL_IDENTIFIER"))
    } else {
        (!machine.is_empty()).then_some((machine, "uname machine"))
    }
}

/// `uname().machine`.
#[cfg(target_os = "ios")]
fn machine() -> Option<String> {
    let mut name = MaybeUninit::<ffi::Utsname>::zeroed();
    // SAFETY: `name` is a writable `utsname`.
    let rc = unsafe { ffi::uname(name.as_mut_ptr()) };
    if rc < 0 {
        return None;
    }
    // SAFETY: a zeroed `utsname` is valid (character arrays only), and the call
    // succeeded.
    let name = unsafe { name.assume_init() };
    Some(crate::parse::lossy(
        &super::c_array_bytes(&name.machine),
        256,
    ))
}

#[cfg(target_os = "ios")]
pub(crate) fn form_factor() -> Detection<FormFactor> {
    let unknown = Detection::new(FormFactor::Unknown, "none", Confidence::None);
    let Some(machine) = machine() else {
        return unknown;
    };
    let simulated = std::env::var("SIMULATOR_MODEL_IDENTIFIER").ok();
    let Some((model, source)) = device_identifier(&machine, simulated.as_deref()) else {
        return unknown;
    };
    match form_from_machine(model) {
        Some(form) if source == "uname machine" => Detection::new(form, source, Confidence::High),
        // Any parent process can set the variable.
        Some(form) => Detection::new(form, source, Confidence::Medium),
        None => Detection::new(FormFactor::Unknown, source, Confidence::Low),
    }
}

/// The text of a registry property: `CFData` holding a C string, or a `CFString`.
#[cfg(target_os = "macos")]
fn registry_text(entry: &Io, key: &str) -> Option<String> {
    let key = Cf::string("firmware", key).ok()?;
    let value = entry.property(&key)?;
    let mut buf = [0u8; 256];
    let bytes = match value.get().data() {
        Some(data) => data.split(|&b| b == 0).next()?,
        None => value.get().text(&mut buf)?,
    };
    crate::parse::smbios::clean(bytes)
}

/// The version out of `system-firmware-version`: `iBoot-11881.41.5` (`mBoot-`
/// from macOS 26.4) is `11881.41.5`, as `system_profiler` shows it.
#[cfg(any(target_os = "macos", test))]
fn firmware_version(text: &str) -> &str {
    match text.split_once('-') {
        Some((_, version)) if version.starts_with(|c: char| c.is_ascii_digit()) => version,
        _ => text,
    }
}

/// The model from `hw.model` and the firmware from the IORegistry: on Apple
/// silicon `system-firmware-version` of `/chosen` (not UEFI, so the kind stays
/// unknown), and the board is the model itself. On Intel the firmware is UEFI,
/// its version is not read (no property could be confirmed), and the board is
/// `board-id` of `IOPlatformExpertDevice`. No serial number is read here.
#[cfg(target_os = "macos")]
pub(crate) fn hardware() -> HardwareFacts {
    let model = sysctl_string(c"hw.model");
    let apple_silicon = native_arch() == Arch::Aarch64;
    let (firmware_kind, firmware_version, board_name) = if apple_silicon {
        let chosen = Io::at_path(c"IODeviceTree:/chosen");
        let version = chosen.and_then(|e| registry_text(&e, "system-firmware-version"));
        let version = version.map(|v| firmware_version(&v).to_owned());
        (None, version, model.clone())
    } else {
        let platform = Io::service(c"IOPlatformExpertDevice");
        let board = platform.and_then(|e| registry_text(&e, "board-id"));
        (Some(FirmwareKind::Uefi), None, board)
    };
    HardwareFacts {
        model_vendor: model.as_ref().map(|_| APPLE.to_owned()),
        model_name: model,
        model_family: None,
        firmware_kind,
        firmware_vendor: Some(APPLE.to_owned()),
        firmware_version,
        firmware_date: None,
        board_vendor: board_name.as_ref().map(|_| APPLE.to_owned()),
        board_name,
        board_version: None,
    }
}

/// The device's identifier as the model (`hw.model` is the board's code name
/// here); iOS keeps its firmware to itself.
#[cfg(target_os = "ios")]
pub(crate) fn hardware() -> HardwareFacts {
    let machine = machine().unwrap_or_default();
    let simulated = std::env::var("SIMULATOR_MODEL_IDENTIFIER").ok();
    let model = device_identifier(&machine, simulated.as_deref()).map(|(m, _)| m.to_owned());
    HardwareFacts {
        model_vendor: model.as_ref().map(|_| APPLE.to_owned()),
        model_name: model,
        ..HardwareFacts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_apple_native_arch() {
        // E-88: an x86-64 process under Rosetta 2 on Apple silicon.
        assert_eq!(native_from(Arch::X86_64, true, true), Arch::Aarch64);
        assert_eq!(native_from(Arch::X86_64, false, false), Arch::X86_64);
        assert_eq!(native_from(Arch::Aarch64, false, true), Arch::Aarch64);
    }

    #[test]
    fn test_detect_apple_virt_checks() {
        let bare = virt_from(false, Some("MacBookPro18,3"), None);
        assert_eq!(bare, Virtualization::default());

        let vz = virt_from(true, Some("VirtualMac2,1"), None);
        assert_eq!(vz.hypervisor, Some(Hypervisor::AppleVirtualization));
        let checks: Vec<_> = vz.evidence.iter().map(|e| e.check).collect();
        assert_eq!(checks, ["kern.hv_vmm_present", "hw.model VirtualMac"]);
        assert!(vz.evidence.iter().all(|e| e.strength == Strength::Strong));

        // A hypervisor that keeps the guest's own model: named by CPUID when it
        // can be, and unnamed when only the sysctl speaks.
        let vmware = virt_from(true, Some("MacPro7,1"), Some(*b"VMwareVMware"));
        assert_eq!(vmware.hypervisor, Some(Hypervisor::Vmware));
        assert_eq!(vmware.evidence.len(), 2);
        let unnamed = virt_from(true, Some("Mac14,2"), None);
        assert_eq!(unnamed.hypervisor, Some(Hypervisor::Other));

        assert_eq!(virt_from(false, None, None).hypervisor, None);
        assert_eq!(cloud(), None);
    }

    #[test]
    fn test_detect_apple_form_from_model() {
        let form = |model, battery| {
            let d = form_from_model(model, battery);
            (d.value, d.confidence)
        };
        for model in ["MacBookPro18,3", "MacBookAir10,1", "MacBook10,1"] {
            assert_eq!(
                form(model, None),
                (FormFactor::Laptop, Confidence::High),
                "{model}"
            );
        }
        for model in ["iMac21,1", "iMacPro1,1", "Macmini9,1", "MacPro7,1"] {
            assert_eq!(
                form(model, Some(false)),
                (FormFactor::Desktop, Confidence::High),
                "{model}"
            );
        }
        // Numbered identifiers (a Mac Studio here) need the battery's answer.
        assert_eq!(
            form("Mac13,1", Some(false)),
            (FormFactor::Desktop, Confidence::Medium)
        );
        assert_eq!(
            form("Mac13,1", None),
            (FormFactor::Unknown, Confidence::Low)
        );
        for model in ["VirtualMac2,1", "SomethingNew1,1", "Mac", "MacThing1,1", ""] {
            assert_eq!(
                form(model, Some(false)),
                (FormFactor::Unknown, Confidence::Low),
                "{model}"
            );
        }
    }

    #[test]
    fn test_detect_apple_form_from_machine() {
        assert_eq!(form_from_machine("iPhone15,2"), Some(FormFactor::Phone));
        assert_eq!(form_from_machine("iPad13,1"), Some(FormFactor::Tablet));
        assert_eq!(form_from_machine("Watch6,1"), Some(FormFactor::Wearable));
        assert_eq!(form_from_machine("AppleTV11,1"), Some(FormFactor::Tv));
        assert_eq!(form_from_machine("arm64"), None);
    }

    #[test]
    fn test_detect_apple_simulator_identifier() {
        // E-91: the simulator's machine is the host's CPU.
        assert_eq!(
            device_identifier("arm64", Some("iPhone15,2")),
            Some(("iPhone15,2", "SIMULATOR_MODEL_IDENTIFIER"))
        );
        assert_eq!(device_identifier("x86_64", None), None);
        // The environment does not speak for a real device.
        assert_eq!(
            device_identifier("iPad13,1", Some("iPhone15,2")),
            Some(("iPad13,1", "uname machine"))
        );
        assert_eq!(device_identifier("", None), None);
    }

    #[test]
    fn test_detect_apple_firmware_version_text() {
        assert_eq!(firmware_version("iBoot-11881.41.5"), "11881.41.5");
        assert_eq!(firmware_version("mBoot-18000.121.3"), "18000.121.3");
        assert_eq!(firmware_version("11881.41.5"), "11881.41.5");
        assert_eq!(firmware_version("odd-text"), "odd-text");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_detect_apple_live_hardware_and_form() {
        let h = hardware();
        assert_eq!(h.model_vendor.as_deref(), Some(APPLE));
        assert!(h.model_name.as_deref().is_some_and(|m| !m.is_empty()));
        assert_eq!(h.firmware_vendor.as_deref(), Some(APPLE));
        if native_arch() == Arch::Aarch64 {
            assert_eq!(h.board_name, h.model_name);
            assert_eq!(h.firmware_kind, None);
        } else {
            assert_eq!(h.firmware_kind, Some(FirmwareKind::Uefi));
        }
        let form = form_factor();
        assert!(!form.source.is_empty());
        if crate::virt::is_vm() {
            assert_eq!(form.value, FormFactor::Unknown);
        }
    }
}
