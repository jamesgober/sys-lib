//! Memory: `hw.physmem`, the `vm.stats.vm` page counters, and `vm.swap_info`
//! (one `struct xswdev` per swap device, read by number).

use core::ffi::c_int;
use std::sync::OnceLock;

use super::{Key, SysctlError, ffi};
use crate::{
    Error, Pressure, Result,
    mem::{HugePages, MemRaw, MemWant, MemoryEvents, MemoryModule, MemorySource, PagingRaw},
};

const WHAT: &str = "memory";

/// The memory module table is in SMBIOS, which only root can read on FreeBSD
/// (`/dev/mem`); the loader copies no module strings into the environment.
pub(crate) fn memory_modules() -> Result<Vec<MemoryModule>> {
    Err(super::unsupported("memory modules"))
}

/// One `vm.stats.vm` page counter.
fn counter(name: &'static core::ffi::CStr, what: &'static str) -> Result<u64> {
    super::sysctl_int(name).map_err(|e| e.into_error(what, name))
}

/// The page counters this reading needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pages {
    free: u64,
    inactive: u64,
}

pub(crate) fn memory(want: MemWant) -> Result<MemRaw> {
    let total = super::sysctl_int(c"hw.physmem").map_err(|e| e.into_error(WHAT, c"hw.physmem"))?;
    let pages = Pages {
        free: counter(c"vm.stats.vm.v_free_count", WHAT)?,
        inactive: counter(c"vm.stats.vm.v_inactive_count", WHAT)?,
    };
    let swap = if want.swap { Some(swap()?) } else { None };
    Ok(raw_from(total, pages, page_size() as u64, swap))
}

/// The public raw reading from the page counters. Available is free plus
/// inactive: the pages the kernel reclaims without writing anything. The
/// laundry queue (dirty pages waiting for the pager) is not available until
/// written. FreeBSD keeps file pages on the same queues as anonymous ones and
/// counts the ZFS ARC as wired, so there is no figure meaning what `cached`
/// means on Linux: `None`.
fn raw_from(total: u64, pages: Pages, page: u64, swap: Option<(u64, u64)>) -> MemRaw {
    let bytes = |n: u64| n.saturating_mul(page);
    MemRaw {
        host_total: total,
        host_available: bytes(pages.free).saturating_add(bytes(pages.inactive)),
        host_unused: Some(bytes(pages.free)),
        cached: None,
        limit: None,
        limited_used: None,
        swap,
        source: Some(MemorySource::Host),
    }
}

/// The most swap devices read: the kernel's `nswapdev` is far below this.
const MAX_SWAP_DEVICES: c_int = 1024;

/// Swap totals in bytes, summed device by device without allocating.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SwapSum {
    total: u64,
    used: u64,
}

impl SwapSum {
    /// Adds one device, in pages of `page` bytes; `None` for a record of
    /// another version. Used beyond the size (counters read at different
    /// instants, threat T5) counts as full; a negative count as zero.
    fn add(&mut self, d: &ffi::Xswdev, page: u64) -> Option<()> {
        if d.version != ffi::XSWDEV_VERSION {
            return None;
        }
        let blocks = u64::try_from(d.nblks).unwrap_or(0);
        let busy = u64::try_from(d.used).unwrap_or(0).min(blocks);
        self.total = self.total.saturating_add(blocks.saturating_mul(page));
        self.used = self.used.saturating_add(busy.saturating_mul(page));
        Some(())
    }

    /// (total, free).
    fn pair(self) -> (u64, u64) {
        (self.total, self.total.saturating_sub(self.used))
    }
}

/// Swap (total, free) in bytes: the sum over `vm.swap_info.N` until the
/// kernel answers `ENOENT` for the next device. No swap is all zero.
fn swap() -> Result<(u64, u64)> {
    const WHAT: &str = "swap";
    let malformed = || Error::Malformed {
        what: WHAT,
        source: "vm.swap_info",
        line: None,
    };
    let mut mib = [0; 8];
    let n = super::name_to_mib(c"vm.swap_info", &mut mib)
        .map_err(|code| super::os_error(WHAT, "vm.swap_info", code))?;
    if n >= mib.len() {
        return Err(malformed());
    }
    let page = page_size() as u64;
    let mut sum = SwapSum::default();
    for index in 0..=MAX_SWAP_DEVICES {
        if index == MAX_SWAP_DEVICES {
            return Err(Error::TooLarge {
                what: "swap devices",
                limit: MAX_SWAP_DEVICES as u64,
            });
        }
        mib[n] = index;
        match super::sysctl_value::<ffi::Xswdev>(Key::Mib(&mib[..=n])) {
            Ok(x) => sum.add(&x, page).ok_or_else(malformed)?,
            Err(SysctlError::Errno(ffi::ENOENT)) => break,
            Err(e) => return Err(e.into_error(WHAT, c"vm.swap_info")),
        }
    }
    Ok(sum.pair())
}

/// The page size (`hw.pagesize`), which is also the unit of the VM counters.
/// Cached: it never changes.
pub(crate) fn page_size() -> usize {
    static PAGE: OnceLock<usize> = OnceLock::new();
    *PAGE.get_or_init(|| {
        super::sysctl_u64(c"hw.pagesize")
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| n.is_power_of_two() && *n >= 4096)
            .unwrap_or(4096)
    })
}

/// FreeBSD has no pressure measure an unprivileged process can read.
pub(crate) fn mem_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

/// FreeBSD keeps no out-of-memory event counters.
pub(crate) fn mem_events() -> Result<Option<MemoryEvents>> {
    Ok(None)
}

/// Pages swapped in and out, in bytes, and the pager's page-in operations
/// (swap and vnode: faults that waited for a disk) as the major faults.
pub(crate) fn paging() -> Result<PagingRaw> {
    const WHAT: &str = "paging activity";
    let page = page_size() as u64;
    let swap_in = counter(c"vm.stats.vm.v_swappgsin", WHAT)?;
    let swap_out = counter(c"vm.stats.vm.v_swappgsout", WHAT)?;
    let swap_faults = counter(c"vm.stats.vm.v_swapin", WHAT)?;
    let vnode_faults = counter(c"vm.stats.vm.v_vnodein", WHAT)?;
    Ok(PagingRaw {
        swap_in: swap_in.saturating_mul(page),
        swap_out: swap_out.saturating_mul(page),
        major_faults: Some(swap_faults.saturating_add(vnode_faults)),
    })
}

/// Superpages are promoted transparently on FreeBSD; there is no pool.
pub(crate) fn huge_pages() -> Result<Option<HugePages>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn test_mem_freebsd_raw_from_counts_pages() {
        let pages = Pages {
            free: 1000,
            inactive: 500,
        };
        let raw = raw_from(16 * GIB, pages, 4096, Some((GIB, GIB / 2)));
        assert_eq!(raw.host_total, 16 * GIB);
        assert_eq!(raw.host_available, 1500 * 4096);
        assert_eq!(raw.host_unused, Some(1000 * 4096));
        assert_eq!(raw.cached, None);
        assert_eq!(raw.swap, Some((GIB, GIB / 2)));
        assert_eq!(raw.source, Some(MemorySource::Host));
        let huge = Pages {
            free: u64::MAX,
            inactive: u64::MAX,
        };
        assert_eq!(raw_from(GIB, huge, 4096, None).host_available, u64::MAX);
    }

    fn device(nblks: c_int, used: c_int) -> ffi::Xswdev {
        ffi::Xswdev {
            version: ffi::XSWDEV_VERSION,
            nblks,
            used,
            ..ffi::Xswdev::default()
        }
    }

    #[test]
    fn test_mem_freebsd_swap_sums_devices() {
        let sum = |devices: &[ffi::Xswdev]| {
            let mut s = SwapSum::default();
            for d in devices {
                s.add(d, 4096)?;
            }
            Some(s.pair())
        };
        assert_eq!(sum(&[]), Some((0, 0)));
        let two = [device(1000, 100), device(500, 0)];
        assert_eq!(sum(&two), Some((1500 * 4096, 1400 * 4096)));
        // T5: used read after the device shrank; negative counts as zero.
        assert_eq!(sum(&[device(10, 20)]), Some((10 * 4096, 0)));
        assert_eq!(sum(&[device(-1, -1)]), Some((0, 0)));
        let mut old = device(10, 0);
        old.version = 1;
        assert_eq!(sum(&[old]), None);
    }

    #[test]
    fn test_mem_freebsd_live_readings_hold_invariants() {
        let raw = memory(MemWant::ALL).unwrap();
        assert!(raw.host_total > 0);
        assert!(raw.host_unused.unwrap() <= raw.host_available);
        let (swap_total, swap_free) = raw.swap.unwrap();
        assert!(swap_free <= swap_total);
        let page = page_size();
        assert!(page.is_power_of_two() && page >= 4096);
        let a = paging().unwrap();
        let b = paging().unwrap();
        assert!(b.swap_in >= a.swap_in && b.swap_out >= a.swap_out);
        assert!(b.major_faults >= a.major_faults);
        assert_eq!(mem_pressure().unwrap(), None);
        assert!(matches!(memory_modules(), Err(Error::Unsupported { .. })));
    }
}
