//! Memory: `hw.memsize`, `host_statistics64(HOST_VM_INFO64)`, `vm.swapusage`,
//! and the kernel's memory-pressure level.

use core::ffi::c_int;
use std::sync::OnceLock;

use super::{SendRight, SysctlError, ffi, kern_error};
use crate::{
    Pressure, PressureLevel, Result,
    mem::{HugePages, MemRaw, MemoryEvents, MemorySource, PagingRaw},
};

pub(crate) fn memory(with_swap: bool) -> Result<MemRaw> {
    const WHAT: &str = "memory";
    let total = super::sysctl_int(c"hw.memsize").map_err(|e| e.into_error(WHAT, c"hw.memsize"))?;
    let vm = vm_statistics(WHAT)?;
    let swap = if with_swap { Some(swap()?) } else { None };
    Ok(raw_from(total, &vm, kernel_page_size(), swap))
}

/// The public raw reading from the VM counters, in pages of `page` bytes.
///
/// The kernel's `free_count` already includes the speculative pages (read
/// ahead, not yet used), so "unused" subtracts them, as `top` does. Available
/// is free plus inactive: both are reclaimed without swapping. Cached is the
/// file-backed pages.
fn raw_from(total: u64, vm: &ffi::VmStatistics64, page: u64, swap: Option<(u64, u64)>) -> MemRaw {
    let bytes = |pages: u32| u64::from(pages).saturating_mul(page);
    MemRaw {
        host_total: total,
        host_available: bytes(vm.free_count).saturating_add(bytes(vm.inactive_count)),
        host_unused: Some(bytes(vm.free_count.saturating_sub(vm.speculative_count))),
        cached: Some(bytes(vm.external_page_count)),
        limit: None,
        limited_used: None,
        swap,
        source: Some(MemorySource::Host),
    }
}

/// The host's VM counters.
fn vm_statistics(what: &'static str) -> Result<ffi::VmStatistics64> {
    let host = SendRight::host(what)?;
    let mut vm = ffi::VmStatistics64::default();
    let mut count = ffi::VM_STATISTICS64_COUNT;
    // SAFETY: `vm` is `count` `natural_t` units of writable memory; the kernel
    // writes at most that many and updates `count`.
    let kr = unsafe {
        ffi::host_statistics64(
            host.0,
            ffi::HOST_VM_INFO64,
            (&raw mut vm).cast::<c_int>(),
            &raw mut count,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(what, "host_statistics64", kr));
    }
    Ok(vm)
}

/// Swap (total, free) from `vm.swapusage`. A kernel without the sysctl has no
/// swap: all zero.
fn swap() -> Result<(u64, u64)> {
    match super::sysctl_value::<ffi::XswUsage>(c"vm.swapusage") {
        Ok(x) => Ok((x.total, x.avail)),
        Err(SysctlError::Errno(ffi::ENOENT)) => Ok((0, 0)),
        Err(e) => Err(e.into_error("swap", c"vm.swapusage")),
    }
}

/// The unit of the `vm_statistics64` counts: the kernel's page size (16 KiB on
/// Apple silicon), which a process under Rosetta 2 does not share, so it is not
/// `page_size()`. Cached: it never changes.
fn kernel_page_size() -> u64 {
    static PAGE: OnceLock<u64> = OnceLock::new();
    *PAGE.get_or_init(|| {
        let from_host = SendRight::host("page size").ok().and_then(|host| {
            let mut size = 0usize;
            // SAFETY: `size` is a writable `vm_size_t`.
            let kr = unsafe { ffi::host_page_size(host.0, &raw mut size) };
            (kr == ffi::KERN_SUCCESS).then_some(size)
        });
        let size = from_host
            .filter(|n| n.is_power_of_two() && *n >= 4096)
            .unwrap_or_else(page_size);
        size as u64
    })
}

/// The page size of this process's address space (`hw.pagesize`): what
/// `sysconf(_SC_PAGESIZE)` gives. Cached: it never changes.
pub(crate) fn page_size() -> usize {
    static PAGE: OnceLock<usize> = OnceLock::new();
    *PAGE.get_or_init(|| {
        super::sysctl_u64(c"hw.pagesize")
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| n.is_power_of_two() && *n >= 4096)
            // Unreachable in practice; the architecture's native size.
            .unwrap_or(if cfg!(target_arch = "aarch64") {
                16_384
            } else {
                4_096
            })
    })
}

/// The kernel's pressure level, as libdispatch reports it: 1 normal, 2 warn, 4
/// critical. A kernel without the sysctl, or a level not in that list, is
/// `None` rather than a guess.
pub(crate) fn mem_pressure() -> Result<Option<Pressure>> {
    const NAME: &core::ffi::CStr = c"kern.memorystatus_vm_pressure_level";
    match super::sysctl_int(NAME) {
        Ok(level) => Ok(pressure_level(level).map(Pressure::from_level)),
        Err(SysctlError::Errno(ffi::ENOENT)) => Ok(None),
        Err(e) => Err(e.into_error("memory pressure", NAME)),
    }
}

fn pressure_level(level: u64) -> Option<PressureLevel> {
    match level {
        1 => Some(PressureLevel::Normal),
        2 => Some(PressureLevel::Warning),
        4 => Some(PressureLevel::Critical),
        _ => None,
    }
}

/// macOS keeps no out-of-memory event counters.
pub(crate) fn mem_events() -> Result<Option<MemoryEvents>> {
    Ok(None)
}

/// Swap-ins and swap-outs in bytes, and page-ins (reads from disk on a fault)
/// as the major faults.
pub(crate) fn paging() -> Result<PagingRaw> {
    let vm = vm_statistics("paging activity")?;
    let page = kernel_page_size();
    Ok(PagingRaw {
        swap_in: vm.swapins.saturating_mul(page),
        swap_out: vm.swapouts.saturating_mul(page),
        major_faults: Some(vm.pageins),
    })
}

/// Superpages are not exposed as a pool on macOS.
pub(crate) fn huge_pages() -> Result<Option<HugePages>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn test_mem_apple_raw_from_counts_pages() {
        let vm = ffi::VmStatistics64 {
            free_count: 1000,
            inactive_count: 500,
            speculative_count: 200,
            external_page_count: 300,
            ..ffi::VmStatistics64::default()
        };
        let raw = raw_from(16 * GIB, &vm, 16_384, Some((GIB, GIB / 2)));
        assert_eq!(raw.host_total, 16 * GIB);
        assert_eq!(raw.host_available, 1500 * 16_384);
        assert_eq!(raw.host_unused, Some(800 * 16_384));
        assert_eq!(raw.cached, Some(300 * 16_384));
        assert_eq!(raw.swap, Some((GIB, GIB / 2)));
        assert_eq!(raw.source, Some(MemorySource::Host));
        assert!(raw.limit.is_none() && raw.limited_used.is_none());
    }

    #[test]
    fn test_mem_apple_raw_from_speculative_above_free_saturates() {
        // T5: counters read at different instants.
        let vm = ffi::VmStatistics64 {
            free_count: 10,
            speculative_count: 20,
            ..ffi::VmStatistics64::default()
        };
        assert_eq!(raw_from(GIB, &vm, 4096, None).host_unused, Some(0));
    }

    #[test]
    fn test_mem_apple_pressure_level_known_values_only() {
        assert_eq!(pressure_level(1), Some(PressureLevel::Normal));
        assert_eq!(pressure_level(2), Some(PressureLevel::Warning));
        assert_eq!(pressure_level(4), Some(PressureLevel::Critical));
        assert_eq!(pressure_level(0), None);
        assert_eq!(pressure_level(3), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_mem_apple_live_readings_hold_invariants() {
        let raw = memory(true).unwrap();
        assert!(raw.host_total > 0);
        assert!(raw.host_unused.unwrap() <= raw.host_available);
        let (swap_total, swap_free) = raw.swap.unwrap();
        assert!(swap_free <= swap_total);
        let page = page_size();
        assert!(page.is_power_of_two() && page >= 4096);
        assert!(kernel_page_size().is_power_of_two());
        assert!(matches!(mem_pressure(), Ok(Some(_) | None)));
        let a = paging().unwrap();
        let b = paging().unwrap();
        assert!(b.swap_in >= a.swap_in && b.swap_out >= a.swap_out);
        assert!(b.major_faults >= a.major_faults);
    }
}
