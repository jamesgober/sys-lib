//! The machine on Linux: DMI's world-readable identity files, the device tree,
//! the form factor, and the memory modules. The serial numbers and the product
//! UUID are mode 0400 and are read only by `crate::linux::device`.

use super::Root;
use crate::{
    Result,
    host::FirmwareKind,
    mem::MemoryModule,
    os::{
        Confidence::{High, Low, Medium, None as NoConfidence},
        Detection, FormFactor, form_from_chassis, form_from_pm_profile,
    },
    parse::{
        self, os_release,
        smbios::{self, clean},
    },
    sys::HardwareFacts,
    virt::Dmi,
};

/// A DMI identity file, cleaned; `None` for a placeholder or a missing file.
fn id(root: &Root, name: &str) -> Option<String> {
    let mut path = String::with_capacity(32);
    path.push_str("/sys/class/dmi/id/");
    path.push_str(name);
    clean(root.text(&path)?.as_bytes())
}

/// The DMI strings detection reads, owned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DmiStrings {
    sys_vendor: Option<String>,
    product: Option<String>,
    product_version: Option<String>,
    board_vendor: Option<String>,
    bios_vendor: Option<String>,
    asset_tag: Option<String>,
}

impl DmiStrings {
    pub(crate) fn view(&self) -> Dmi<'_> {
        Dmi {
            sys_vendor: self.sys_vendor.as_deref().unwrap_or(""),
            product: self.product.as_deref().unwrap_or(""),
            product_version: self.product_version.as_deref().unwrap_or(""),
            board_vendor: self.board_vendor.as_deref().unwrap_or(""),
            bios_vendor: self.bios_vendor.as_deref().unwrap_or(""),
            asset_tag: self.asset_tag.as_deref().unwrap_or(""),
        }
    }
}

pub(crate) fn dmi(root: &Root) -> DmiStrings {
    DmiStrings {
        sys_vendor: id(root, "sys_vendor"),
        product: id(root, "product_name"),
        product_version: id(root, "product_version"),
        board_vendor: id(root, "board_vendor"),
        bios_vendor: id(root, "bios_vendor"),
        asset_tag: id(root, "chassis_asset_tag"),
    }
}

/// A device-tree string: NUL-terminated.
fn device_tree(root: &Root, path: &str) -> Option<String> {
    let text = root.text(path)?;
    clean(text.split('\0').next()?.as_bytes())
}

pub(crate) fn facts(root: &Root) -> HardwareFacts {
    let vendor = id(root, "sys_vendor");
    let bios_vendor = id(root, "bios_vendor");
    let bios_version = id(root, "bios_version");
    let bios_date = id(root, "bios_date").map(|d| smbios::iso_date(&d));
    let dmi_present = vendor.is_some() || bios_vendor.is_some() || bios_version.is_some();
    let kind = if root.exists("/sys/firmware/efi") {
        Some(FirmwareKind::Uefi)
    } else {
        dmi_present.then_some(FirmwareKind::Bios)
    };
    HardwareFacts {
        model_name: id(root, "product_name").or_else(|| device_tree(root, "/proc/device-tree/model")),
        model_vendor: vendor,
        model_family: id(root, "product_family"),
        firmware_kind: kind,
        firmware_vendor: bios_vendor,
        firmware_version: bios_version,
        firmware_date: bios_date,
        board_vendor: id(root, "board_vendor"),
        board_name: id(root, "board_name"),
        board_version: id(root, "board_version"),
    }
}

/// systemd's `CHASSIS=` values.
fn form_from_name(name: &str) -> Option<FormFactor> {
    Some(match name {
        "desktop" => FormFactor::Desktop,
        "laptop" => FormFactor::Laptop,
        "convertible" => FormFactor::Convertible,
        "tablet" => FormFactor::Tablet,
        "handset" => FormFactor::Phone,
        "watch" => FormFactor::Wearable,
        "server" => FormFactor::Server,
        "embedded" => FormFactor::Embedded,
        "vm" | "container" => FormFactor::Unknown,
        _ => return None,
    })
}

/// The form factor: first answer wins. Inside a VM the firmware describes
/// virtual hardware, so only the administrator's word counts (E-79).
pub(crate) fn form_factor(root: &Root, in_vm: bool, has_battery: bool) -> Detection<FormFactor> {
    // E-80: the administrator overrides the firmware, as systemd honours it.
    let chassis = root.read(
        "/etc/machine-info",
        1 << 16,
        "form factor",
        "/etc/machine-info",
    );
    if let Ok(data) = chassis {
        let named = os_release::pairs(&data)
            .find(|(k, _)| *k == b"CHASSIS")
            .and_then(|(_, v)| form_from_name(&v));
        if let Some(form) = named {
            return Detection::new(form, "/etc/machine-info", High);
        }
    }
    if in_vm {
        return Detection::new(FormFactor::Unknown, "virtual machine", NoConfidence);
    }
    let number = |path: &str| -> Option<u8> {
        let text = root.text(path)?;
        u8::try_from(parse::parse_u64(text.trim().as_bytes())?).ok()
    };
    if let Some(form) = number("/sys/class/dmi/id/chassis_type").and_then(form_from_chassis) {
        return Detection::new(form, "DMI chassis type", High);
    }
    if let Some(form) = number("/sys/firmware/acpi/pm_profile").and_then(form_from_pm_profile) {
        return Detection::new(form, "ACPI power management profile", Medium);
    }
    let tree = root
        .text("/proc/device-tree/chassis-type")
        .and_then(|t| form_from_name(t.split('\0').next()?));
    if let Some(form) = tree {
        return Detection::new(form, "device tree chassis type", High);
    }
    if has_battery {
        return Detection::new(FormFactor::Laptop, "battery", Low);
    }
    Detection::new(FormFactor::Unknown, "none", NoConfidence)
}

/// The installed modules: the raw table is root-only, so `PermissionDenied`
/// names what is needed.
pub(crate) fn modules(root: &Root) -> Result<Vec<MemoryModule>> {
    const TABLE: &str = "/sys/firmware/dmi/tables/DMI";
    let table = root.read(TABLE, super::FILE_LIMIT, "memory modules", TABLE)?;
    Ok(smbios::memory_devices(&table)
        .into_iter()
        .map(crate::mem::module_from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};
    use crate::linux::hostile::Tree;

    #[test]
    fn test_hardware_wsl2_has_no_dmi() {
        // E-41.
        let root = fixtures::root("wsl2");
        let f = facts(&root);
        assert_eq!(f.model_name, None);
        assert_eq!(f.firmware_kind, None);
        let d = form_factor(&root, true, false);
        assert_eq!(d.value, FormFactor::Unknown);
    }

    #[test]
    fn test_hardware_form_factor_order() {
        let t = Tree::new("form");
        t.put("/sys/class/dmi/id/chassis_type", b"10\n");
        t.put("/sys/firmware/acpi/pm_profile", b"1\n");
        let d = form_factor(&t.root(), false, false);
        assert_eq!((d.value, d.confidence), (FormFactor::Laptop, High));
        // E-79: a VM's chassis describes virtual hardware.
        assert_eq!(form_factor(&t.root(), true, false).value, FormFactor::Unknown);
        // E-80: the administrator wins, in a VM too.
        t.put("/etc/machine-info", b"CHASSIS=\"server\"\n");
        let d = form_factor(&t.root(), true, false);
        assert_eq!((d.value, d.source), (FormFactor::Server, "/etc/machine-info"));
    }

    #[test]
    fn test_hardware_form_factor_fallbacks() {
        let t = Tree::new("form2");
        t.put("/sys/class/dmi/id/chassis_type", b"1\n"); // "Other": next source
        t.put("/sys/firmware/acpi/pm_profile", b"4\n");
        assert_eq!(form_factor(&t.root(), false, false).value, FormFactor::Server);
        let t = Tree::new("form3");
        assert_eq!(form_factor(&t.root(), false, false).value, FormFactor::Unknown);
        let d = form_factor(&t.root(), false, true);
        assert_eq!((d.value, d.confidence), (FormFactor::Laptop, Low));
        t.put("/proc/device-tree/chassis-type", b"handset\0");
        assert_eq!(form_factor(&t.root(), false, false).value, FormFactor::Phone);
    }

    #[test]
    fn test_hardware_firmware_and_board() {
        let t = Tree::new("hw");
        t.put("/sys/class/dmi/id/sys_vendor", b"LENOVO\n");
        t.put("/sys/class/dmi/id/product_name", b"21HM\n");
        t.put("/sys/class/dmi/id/product_family", b"ThinkPad X1\n");
        t.put("/sys/class/dmi/id/bios_vendor", b"LENOVO\n");
        t.put("/sys/class/dmi/id/bios_version", b"N3XET60W\n");
        t.put("/sys/class/dmi/id/bios_date", b"03/14/2025\n");
        t.put("/sys/class/dmi/id/board_name", b"To be filled by O.E.M.\n");
        let f = facts(&t.root());
        assert_eq!(f.model_name.as_deref(), Some("21HM"));
        assert_eq!(f.firmware_kind, Some(FirmwareKind::Bios));
        assert_eq!(f.firmware_date.as_deref(), Some("2025-03-14"));
        assert_eq!(f.board_name, None);
        t.mkdir("/sys/firmware/efi");
        assert_eq!(facts(&t.root()).firmware_kind, Some(FirmwareKind::Uefi));
    }

    #[test]
    fn test_hardware_raspberry_pi_model() {
        let t = Tree::new("pi");
        t.put("/proc/device-tree/model", b"Raspberry Pi 5 Model B Rev 1.0\0");
        assert_eq!(
            facts(&t.root()).model_name.as_deref(),
            Some("Raspberry Pi 5 Model B Rev 1.0")
        );
    }

    #[test]
    fn test_hardware_modules_without_the_table() {
        let t = Tree::new("mods");
        assert!(modules(&t.root()).is_err());
        let table = crate::parse::smbios::tests::sample();
        t.put("/sys/firmware/dmi/tables/DMI", &table);
        assert!(modules(&t.root()).unwrap().is_empty());
    }
}
