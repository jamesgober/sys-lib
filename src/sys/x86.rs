//! CPUID for the detection that reads it, on every x86 backend. `__cpuid` needs
//! no privilege and touches no memory; every leaf read here is checked against
//! the highest leaf the processor reports.

#![cfg_attr(
    not(any(
        all(
            any(target_os = "linux", target_os = "android"),
            target_pointer_width = "64"
        ),
        all(
            any(target_os = "macos", target_os = "ios"),
            target_pointer_width = "64"
        ),
        all(target_os = "freebsd", target_pointer_width = "64"),
        windows
    )),
    allow(dead_code)
)]

#[cfg(target_arch = "x86")]
use core::arch::x86::{__cpuid, CpuidResult};
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{__cpuid, CpuidResult};

/// The bit CPUID leaf 1 sets in ECX under a hypervisor.
const HYPERVISOR_BIT: u32 = 1 << 31;
const HYPERVISOR_LEAF: u32 = 0x4000_0000;
/// Hyper-V's partition privilege flags (Hypervisor TLFS).
const HYPERV_FEATURES_LEAF: u32 = 0x4000_0003;
/// `CreatePartitions`, set only for the root partition: bit 0 of EBX.
const CREATE_PARTITIONS: u32 = 1;

// `__cpuid` is `unsafe` on the oldest supported toolchain and safe on newer ones.
#[allow(unused_unsafe)]
fn cpuid(leaf: u32) -> CpuidResult {
    // SAFETY: CPUID exists on every x86-64 processor and every x86 processor
    // Rust supports; it reads no memory.
    unsafe { __cpuid(leaf) }
}

/// The hypervisor's vendor signature, when CPUID says there is one.
pub(crate) fn hypervisor_signature() -> Option<[u8; 12]> {
    if cpuid(0).eax < 1 || cpuid(1).ecx & HYPERVISOR_BIT == 0 {
        return None;
    }
    let r = cpuid(HYPERVISOR_LEAF);
    let mut out = [0u8; 12];
    out[..4].copy_from_slice(&r.ebx.to_le_bytes());
    out[4..8].copy_from_slice(&r.ecx.to_le_bytes());
    out[8..12].copy_from_slice(&r.edx.to_le_bytes());
    Some(out)
}

/// Whether this is Hyper-V's root partition: a Windows host with Hyper-V, WSL 2,
/// or VBS, not a VM (E-46).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_hyperv_root() -> bool {
    hypervisor_signature().as_ref() == Some(b"Microsoft Hv")
        && cpuid(HYPERVISOR_LEAF).eax >= HYPERV_FEATURES_LEAF
        && cpuid(HYPERV_FEATURES_LEAF).ebx & CREATE_PARTITIONS != 0
}

/// The vendor string of CPUID leaf 0 (`GenuineIntel`, `AuthenticAMD`), for a
/// backend whose OS does not report it.
#[cfg_attr(not(target_os = "freebsd"), allow(dead_code))]
pub(crate) fn vendor_id() -> [u8; 12] {
    let r = cpuid(0);
    let mut out = [0u8; 12];
    out[..4].copy_from_slice(&r.ebx.to_le_bytes());
    out[4..8].copy_from_slice(&r.edx.to_le_bytes());
    out[8..12].copy_from_slice(&r.ecx.to_le_bytes());
    out
}

/// The processor's family, model, and stepping: CPUID leaf 1 EAX.
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn signature() -> u32 {
    if cpuid(0).eax < 1 { 0 } else { cpuid(1).eax }
}

/// The extended leaf with the power-management flags.
const POWER_LEAF: u32 = 0x8000_0007;
/// `InvariantTSC`: bit 8 of EDX in the power leaf (Intel SDM, AMD APM).
const INVARIANT_TSC: u32 = 1 << 8;

/// Whether the time-stamp counter runs at a constant rate through frequency
/// changes and idle states; `None` when the processor has no power leaf.
pub(crate) fn invariant_tsc() -> Option<bool> {
    if cpuid(0x8000_0000).eax < POWER_LEAF {
        return None;
    }
    Some(cpuid(POWER_LEAF).edx & INVARIANT_TSC != 0)
}
