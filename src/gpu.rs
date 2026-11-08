//! Graphics adapters.
//!
//! What the OS says about each adapter: its name, vendor, kind, PCI ids,
//! dedicated memory, and driver. No usage or memory in use: those need vendor
//! libraries (NVML, ROCm SMI, DXGI), which sys-lib does not load.
//!
//! ```
//! use sys_lib as sys;
//!
//! for g in sys::gpu::list().unwrap_or_default() {
//!     println!("{} ({:?}, {:?}) memory {:?}", g.name, g.vendor, g.kind, g.memory);
//! }
//! ```

use crate::{Bytes, Result};

/// One graphics adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Gpu {
    /// Its name: "NVIDIA GeForce RTX 4070", "Apple M2". On Linux without a
    /// `pci.ids` database, the vendor and the device id: "NVIDIA GPU 0x2786".
    pub name: String,
    /// Who made it.
    pub vendor: GpuVendor,
    /// Built in, on its own card, or emulated.
    pub kind: GpuKind,
    /// The PCI vendor id.
    pub vendor_id: Option<u16>,
    /// The PCI device id.
    pub device_id: Option<u16>,
    /// Dedicated video memory, when the OS reports it.
    pub memory: Option<Bytes>,
    /// The driver: `amdgpu`, `i915`, a Windows driver version.
    pub driver: Option<String>,
}

/// Who made a graphics adapter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum GpuVendor {
    /// NVIDIA.
    Nvidia,
    /// AMD.
    Amd,
    /// Intel.
    Intel,
    /// Apple.
    Apple,
    /// Qualcomm.
    Qualcomm,
    /// Arm (Mali).
    Arm,
    /// Microsoft (the basic display adapter, Hyper-V).
    Microsoft,
    /// VMware.
    Vmware,
    /// Anyone else.
    Other,
}

impl GpuVendor {
    /// The vendor for a PCI vendor id.
    #[must_use]
    pub const fn from_pci(id: u16) -> GpuVendor {
        match id {
            0x10de => Self::Nvidia,
            0x1002 | 0x1022 => Self::Amd,
            0x8086 => Self::Intel,
            0x106b => Self::Apple,
            0x5143 | 0x17cb => Self::Qualcomm,
            0x13b5 => Self::Arm,
            0x1414 => Self::Microsoft,
            0x15ad => Self::Vmware,
            _ => Self::Other,
        }
    }

    /// Its name: `"NVIDIA"`, `"AMD"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA",
            Self::Amd => "AMD",
            Self::Intel => "Intel",
            Self::Apple => "Apple",
            Self::Qualcomm => "Qualcomm",
            Self::Arm => "Arm",
            Self::Microsoft => "Microsoft",
            Self::Vmware => "VMware",
            Self::Other => "Other",
        }
    }
}

/// Where a graphics adapter is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum GpuKind {
    /// Part of the CPU or the system chip.
    Integrated,
    /// A card or chip of its own, with its own memory.
    Discrete,
    /// Emulated by a hypervisor, or a display driver with no hardware.
    Virtual,
    /// Not known.
    Unknown,
}

/// The kind a vendor's adapter usually is, for platforms that do not say:
/// Intel and Apple are integrated, NVIDIA discrete, the hypervisors' virtual;
/// AMD makes both, so `Unknown` unless the caller knows more.
#[allow(dead_code)] // INTERIM until every backend uses it
pub(crate) const fn kind_of(vendor: GpuVendor) -> GpuKind {
    match vendor {
        GpuVendor::Intel | GpuVendor::Apple | GpuVendor::Qualcomm | GpuVendor::Arm => {
            GpuKind::Integrated
        }
        GpuVendor::Nvidia => GpuKind::Discrete,
        GpuVendor::Microsoft | GpuVendor::Vmware => GpuKind::Virtual,
        GpuVendor::Amd | GpuVendor::Other => GpuKind::Unknown,
    }
}

/// Every graphics adapter the OS lists, in its order.
///
/// # Errors
///
/// `Unsupported` on iOS and targets without a backend; `Io` or `Os` when the
/// OS's list cannot be read.
pub fn list() -> Result<Vec<Gpu>> {
    crate::sys::gpus()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_vendor_from_pci() {
        assert_eq!(GpuVendor::from_pci(0x10de), GpuVendor::Nvidia);
        assert_eq!(GpuVendor::from_pci(0x1002), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_pci(0x8086), GpuVendor::Intel);
        assert_eq!(GpuVendor::from_pci(0x1234), GpuVendor::Other);
        assert_eq!(kind_of(GpuVendor::Nvidia), GpuKind::Discrete);
        assert_eq!(kind_of(GpuVendor::Amd), GpuKind::Unknown);
    }

    #[test]
    fn test_gpu_live_list_or_unavailable() {
        match list() {
            Ok(list) => {
                for g in list {
                    assert!(!g.name.is_empty(), "{g:?}");
                }
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
    }
}
